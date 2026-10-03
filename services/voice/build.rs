// libwebrtc (pulled in by the `call-worker` feature) uses Objective-C
// categories on Apple targets. webrtc-sys prints `-ObjC`, but cargo doesn't
// forward a dependency's link args to the final binary, so without this the
// binary aborts at startup (`+[NSString stringForStdString:]: unrecognized
// selector`).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let apple = matches!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("macos") | Ok("ios")
    );
    if apple && std::env::var_os("CARGO_FEATURE_CALL_WORKER").is_some() {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
