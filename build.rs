fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/fonts/LICENSE.txt");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon_with_id("assets/app.ico", "101")
            .append_rc_content(&format!(
                "301 RCDATA \"{}/assets/fonts/LICENSE.txt\"",
                std::env::var("CARGO_MANIFEST_DIR")
                    .unwrap()
                    .replace('\\', "/")
            ))
            .compile()
            .expect("Windows 아이콘 리소스를 컴파일하지 못했습니다");
    }
}
