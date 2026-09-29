//! Windows: embed the app icon and version info into bro.exe (shows in Explorer, the taskbar and Windows Terminal).

fn main() {
    println!("cargo:rerun-if-changed=assets/bro.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/bro.ico");
        res.set("FileDescription", "bro — agentic terminal workspace");
        res.set("ProductName", "bro");
        if let Err(e) = res.compile() {
            // no resource compiler (e.g. a bare toolchain): build without the icon rather than fail
            println!("cargo:warning=bro.exe built without its icon: {e}");
        }
    }
}
