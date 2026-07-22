fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/app.ico")
            .compile()
            .expect("Windows 아이콘 리소스를 컴파일하지 못했습니다");
    }
}
