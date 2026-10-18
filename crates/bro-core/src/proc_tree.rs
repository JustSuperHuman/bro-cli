//! Own the lifetime of a session and every process it starts.
//!
//! Windows uses a non-inheritable, kill-on-close Job Object. ConPTY assigns the job during
//! CreateProcess; ordinary commands start suspended and join before their first instruction.
//! A root-process watcher ends the job when the root exits, even if descendants keep its output
//! pipes open. Dropping the owner or terminating Bro also ends the job. No PID walk or executable
//! subsystem heuristic can lose descendants after a launcher exits.
//!
//! Unix children lead their own process group (the PTY creates a session; ordinary commands use
//! process_group). Cleanup signals that group even after its leader has exited. Processes that
//! deliberately create another session remain outside that group.

use std::io;
use std::process::{Child, Command};

/// Keep this owner until the session or task ends. Dropping it terminates its process tree.
pub struct ProcessTree {
    root: Option<u32>,
    #[cfg(windows)]
    job: std::sync::Arc<imp::Job>,
}

impl ProcessTree {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            root: None,
            #[cfg(windows)]
            job: std::sync::Arc::new(imp::Job::new()?),
        })
    }

    /// Configure a background command before spawning it. Call `attach` immediately afterwards.
    pub fn configure_command(&self, command: &mut Command) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // The child must not execute or start descendants before job assignment.
            command.creation_flags(0x0800_0004); // CREATE_NO_WINDOW | CREATE_SUSPENDED
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(not(any(windows, unix)))]
        let _ = command;
    }

    /// Assign a background child, start its exit watcher, then let it execute.
    pub fn attach(&mut self, pid: u32) -> io::Result<()> {
        self.root = Some(pid);
        #[cfg(windows)]
        imp::attach(&self.job, pid, true)?;
        Ok(())
    }

    /// Watch a PTY child that was born in this job (or its own Unix session).
    pub fn watch_pty(&mut self, pid: u32) -> io::Result<()> {
        self.root = Some(pid);
        #[cfg(windows)]
        imp::attach(&self.job, pid, false)?;
        Ok(())
    }

    /// Borrow the job for ConPTY's process-creation attribute. The owner must outlive the spawn.
    #[cfg(windows)]
    pub fn job_handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        use std::os::windows::io::AsHandle;
        self.job.0.as_handle()
    }

    /// Spawn a background command with cleanup on every return path.
    pub fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        let mut tree = Self::new()?;
        tree.configure_command(command);
        let mut child = command.spawn()?;
        if let Err(error) = tree.attach(child.id()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, tree))
    }

    /// End all owned processes. Repeated calls cannot target a reused PID.
    pub fn terminate(&mut self) {
        #[cfg(windows)]
        self.job.terminate();
        #[cfg(unix)]
        if let Some(root) = self.root.take().and_then(|pid| i32::try_from(pid).ok()).filter(|pid| *pid > 1) {
            // The group can outlive its leader, so getpgid(root) is not a useful guard here.
            unsafe { libc::killpg(root, libc::SIGKILL) };
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::Arc;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, OpenThread, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, ResumeThread, THREAD_SUSPEND_RESUME, TerminateProcess,
        WaitForSingleObject,
    };

    pub struct Job(pub OwnedHandle);

    impl Job {
        pub fn new() -> io::Result<Self> {
            // NULL security attributes make this handle non-inheritable. Children must never keep
            // the last job handle alive after Bro exits.
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe {
                SetInformationJobObject(
                    raw,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of_val(&limits) as u32,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }

        pub fn terminate(&self) {
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        }
    }

    pub fn attach(job: &Arc<Job>, pid: u32, suspended: bool) -> io::Result<()> {
        let raw = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        let result = (|| {
            if suspended && unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), raw) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let watching = job.clone();
            let watched_process = process.try_clone()?;
            std::thread::Builder::new().name("bro-process-exit".into()).spawn(move || {
                if unsafe { WaitForSingleObject(watched_process.as_raw_handle(), INFINITE) } == WAIT_OBJECT_0 {
                    watching.terminate();
                }
            })?;
            if suspended {
                resume(pid)?;
            }
            Ok(())
        })();
        if result.is_err() {
            // Also covers assignment failure: an unassigned suspended child must never survive.
            unsafe { TerminateProcess(raw, 1) };
            job.terminate();
        }
        result
    }

    fn resume(pid: u32) -> io::Result<()> {
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut next = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
        while next {
            if entry.th32OwnerProcessID == pid {
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let result = unsafe { ResumeThread(thread) };
                let error = io::Error::last_os_error();
                unsafe { CloseHandle(thread) };
                return if result == u32::MAX { Err(error) } else { Ok(()) };
            }
            next = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "the suspended child's primary thread was not found"))
    }
}

#[cfg(test)]
mod tests;
