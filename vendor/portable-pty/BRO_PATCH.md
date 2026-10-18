# Bro's portable-pty patch

This is portable-pty 0.9.0 under its original MIT license.

Bro adds `CommandBuilder::set_windows_job_handle` and passes that job through
`PROC_THREAD_ATTRIBUTE_JOB_LIST` alongside the ConPTY attribute. Windows assigns
the child to its session job during `CreateProcessW`, before any child code runs.
The caller owns the job handle and keeps it open through process creation.

The remaining source and examples match the published crate. Unix spawning is unchanged.
