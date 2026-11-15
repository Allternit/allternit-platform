#![cfg(windows)]
use files_win_cfapi_spike::*;

#[test]
fn register_placeholders_hydrate_unregister() {
    SERVER.set(fake_tree()).unwrap();
    let root = temp_root();
    let _ = std::fs::remove_dir_all(&root);

    // (1) register
    register(&root).expect("CfRegisterSyncRoot");
    println!("[cfapi-spike] STEP1 CfRegisterSyncRoot ok: {}", root.display());

    // connect first so callbacks are live
    let key = connect(&root).expect("CfConnectSyncRoot");
    println!("[cfapi-spike] STEP3a CfConnectSyncRoot ok");

    // (2) placeholders
    let n = create_tree(&root).expect("CfCreatePlaceholders");
    assert_eq!(n, 7, "placeholders created");
    println!("[cfapi-spike] STEP2 CfCreatePlaceholders ok: {n} entries");
    let names: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
    println!("[cfapi-spike] root listing: {names:?}");

    // (3) hydrate on open: read bytes back through the normal filesystem API
    for (rel, expected) in fake_tree() {
        let got = std::fs::read(root.join(&rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        assert_eq!(got, expected, "bytes for {rel}");
        println!("[cfapi-spike] STEP3 hydrated {rel}: {} bytes exact match", got.len());
    }

    // (4) clean unregister
    disconnect_and_unregister(key, &root).expect("unregister");
    println!("[cfapi-spike] STEP4 CfDisconnectSyncRoot + CfUnregisterSyncRoot ok");
    let _ = std::fs::remove_dir_all(&root);
}
