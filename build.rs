//! Embeds the application icon and version info into the Windows executable.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=quotty.rc");
        println!("cargo:rerun-if-changed=assets/quotty.ico");
        embed_resource::compile("quotty.rc", embed_resource::NONE);
    }
}
