//! Own the lifetime of a session and every process it starts.
//!
//! Windows uses a non-inheritable, kill-on-close Job Object. ConPTY and ordinary commands
//! start suspended and join the job before their first instruction.
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

    /// Borrow the job for ConPTY's suspended-child assignment. The owner must outlive the spawn.
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

impl ProcessTree {
    /// A handle for looking through this tree while it runs (the Slop Collector, [`crate::slop`]).
    pub fn scope(&self) -> Scope {
        Scope {
            root: self.root,
            #[cfg(windows)]
            job: self.job.clone(),
        }
    }
}

/// What the Slop Collector looks through: the processes a tree holds now, and ending one only while the tree still
/// holds it. Cheap to clone. Hold it only while looking: on Windows it keeps the job open.
#[derive(Clone)]
pub struct Scope {
    root: Option<u32>,
    #[cfg(windows)]
    job: std::sync::Arc<imp::Job>,
}

impl Scope {
    /// The tree's first process (the session's program).
    pub fn root(&self) -> Option<u32> {
        self.root
    }

    /// Every process the tree holds now, however it was started and whoever started it (Windows: the job's
    /// members, which no launcher exiting can lose). Empty where bro can't tell.
    pub fn pids(&self) -> Vec<u32> {
        #[cfg(windows)]
        return imp::members(&self.job);
        #[cfg(not(windows))]
        Vec::new()
    }

    /// End `pid`, but only while this tree still holds it and it's the process that started at `started` (as
    /// [`crate::slop`] read it), so a reused PID is never touched. True when it was ended.
    pub fn end(&self, pid: u32, started: u64) -> bool {
        #[cfg(windows)]
        return imp::end(&self.job, pid, started);
        #[cfg(not(windows))]
        {
            let _ = (pid, started);
            false
        }
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

    /// The job's processes now.
    pub fn members(job: &Job) -> Vec<u32> {
        use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, GetLastError};
        use windows_sys::Win32::System::JobObjects::{JOBOBJECT_BASIC_PROCESS_ID_LIST, JobObjectBasicProcessIdList, QueryInformationJobObject};
        let word = std::mem::size_of::<usize>();
        let head = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList);
        let mut room = 256usize;
        for _ in 0..4 {
            let mut buf = vec![0usize; (head + room * word).div_ceil(word)];
            let ok = unsafe { QueryInformationJobObject(job.0.as_raw_handle(), JobObjectBasicProcessIdList, buf.as_mut_ptr().cast(), (buf.len() * word) as u32, std::ptr::null_mut()) };
            // SAFETY: the buffer is word-aligned and at least the header long; the list sits after the header
            let list = unsafe { &*(buf.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST) };
            let assigned = list.NumberOfAssignedProcesses as usize;
            if ok == 0 {
                if unsafe { GetLastError() } == ERROR_MORE_DATA && assigned > room {
                    room = assigned + 64;
                    continue;
                }
                return vec![];
            }
            let n = (list.NumberOfProcessIdsInList as usize).min(room);
            if assigned > n {
                room = assigned + 64;
                continue;
            }
            let ids = unsafe { std::slice::from_raw_parts((buf.as_ptr() as *const u8).add(head) as *const usize, n) };
            return ids.iter().map(|&p| p as u32).collect();
        }
        vec![]
    }

    /// End `pid` while the job holds it and it started at `started` (a FILETIME, as `GetProcessTimes` says).
    pub fn end(job: &Job, pid: u32, started: u64) -> bool {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{GetProcessTimes, PROCESS_QUERY_LIMITED_INFORMATION};
        let raw = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            return false;
        }
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut inside = 0;
        if unsafe { IsProcessInJob(process.as_raw_handle(), job.0.as_raw_handle(), &mut inside) } == 0 || inside == 0 {
            return false;
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        if unsafe { GetProcessTimes(process.as_raw_handle(), &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
            return false;
        }
        if ((created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64) != started {
            return false;
        }
        unsafe { TerminateProcess(process.as_raw_handle(), 1) != 0 }
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
