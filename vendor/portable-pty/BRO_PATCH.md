# Bro's portable-pty patch

This is portable-pty 0.9.0 under its original MIT license.

Bro adds `CommandBuilder::set_windows_job_handle`. When a job is requested, the
ConPTY child starts with `CREATE_SUSPENDED`, joins the supplied session job, and
only then has its primary thread resumed. Assignment or resume failure terminates
the suspended child. The caller owns the job handle and keeps it open through
process creation. No child code runs before job ownership is established.

This avoids `PROC_THREAD_ATTRIBUTE_JOB_LIST`, which rejects Microsoft Store
execution aliases and packaged executables such as Store-installed PowerShell.
Trying that attribute first can also leave the job incompatible with a subsequent
activation, so the suspended launch is used from the outset, not as a retry.
Job-owned launches also set `PROCESS_CREATION_DESKTOP_APP_BREAKAWAY_DISABLE_PROCESS_TREE`:
without it, packaged PowerShell can launch children outside the job hierarchy,
leaving descendants alive after the terminal closes. Windows still resolves
aliases normally: there are no cached package-version paths, command-shell
wrappers, or fallback changes to the user's shell.

Launches without a job and Unix spawning are unchanged. The remaining source and
examples match the published crate.
