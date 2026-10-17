# Upstream PR draft for nab138/isideload

Not sent. Text and patch are ready for whoever opens it (needs a GitHub account
decision by the orchestrator). Our crate does not depend on this landing: it
works around the gap with isideload's public lower-level APIs (README, "The
.xctest fix").

---

**Title:** Sign XCTest bundles in PlugIns/ with the host app's identity

**Body:**

`Bundle::new` loads every directory in `PlugIns/` as an app extension. That's
right for `.appex`, but an XCUITest runner (`Foo-Runner.app`) also carries its
test bundle there: `PlugIns/FooUITests.xctest`. Xcode signs that bundle with
the runner's own bundle identifier and team, no entitlements and no embedded
profile; it never gets an App ID of its own.

Today, with a runner whose `.xctest` carries the runner's id (as Xcode and most
re-signing scripts leave it):

- `Application::update_bundle_id` bails: *"Extension FooUITests is not part of
  the main app bundle identifier"*, because the id doesn't extend the main id
  with a suffix.
- If the `.xctest` id does extend it (`<runner>.something`), `register_app_ids`
  registers an extra App ID for it (spending one of a free account's 10 per
  week) and the test bundle gets signed as a separate identity, after which the
  runner installs but crashes at launch.

This PR treats `*.xctest` plug-ins as part of the host: they take the main
app's new bundle id and are skipped when registering App IDs/profiles. With no
entry in `entitlements_by_bundle_id` / `embedded_mobileprovisions_by_bundle_id`,
apple-codesign-quick then signs the nested bundle with the main identifier and
team, empty entitlements and no profile, matching Xcode.

Tested by: signing an XCUITest runner (iCTRL) for a free account and launching
it with `xcodebuild test-without-building` / go-ios `runtest`.

```diff
--- a/src/sideload/bundle.rs
+++ b/src/sideload/bundle.rs
@@ impl Bundle {
+    /// XCTest bundles (`PlugIns/*.xctest`) run inside their host app and share
+    /// its identity; they are not app extensions.
+    pub fn is_xctest(&self) -> bool {
+        self.bundle_dir.extension().is_some_and(|ext| ext == "xctest")
+    }
+
     pub fn app_extensions(&self) -> &[Bundle] {
--- a/src/sideload/application.rs
+++ b/src/sideload/application.rs
@@ pub fn update_bundle_id(
         let extensions = self.bundle.app_extensions_mut();
         for ext in extensions.iter_mut() {
+            if ext.is_xctest() {
+                // Test bundles share the host app's bundle id.
+                ext.set_bundle_identifier(main_app_id_str);
+                continue;
+            }
             if let Some(id) = ext.bundle_identifier() {
@@ pub async fn register_app_ids(
-        let extension_refs: Vec<_> = self.bundle.app_extensions().iter().collect();
+        // XCTest bundles use the host's App ID and profile.
+        let extension_refs: Vec<_> = self
+            .bundle
+            .app_extensions()
+            .iter()
+            .filter(|ext| !ext.is_xctest())
+            .collect();
```

(`sideloader.rs` needs no change: it downloads profiles per registered App ID,
excluding the main one, so the `.xctest` correctly gets none.)
