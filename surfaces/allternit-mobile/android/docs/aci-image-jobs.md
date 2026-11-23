# Images on this Android phone

Choose Image in the composer, enter a prompt, then confirm the exact prompt. The app opens ChatGPT on this phone and selects Create image in a regular empty chat. Its website sign-in is separate from OAuth chat authorization. A connected computer and a model tool are not required. If ChatGPT sign-in, verification, image mode, or download cannot run in this WebView, the app gives an actionable error. It does not send the request to another device.

Return to chat hides the browser. Opening another chat keeps the image job bound to its original chat. Use Open image browser to return, Resume after a sign-in or verification step, or Stop image to cancel observation. ChatGPT may finish a prompt already sent. Android suspension pauses observation; return to the app to reconnect. A restarted request never sends again automatically.

The finished image card offers Download, Open, and Copy prompt. Outputs in the chat header lists this account's images saved on this phone and opens their original chats, including after restart. Download exports the actual PNG through Android's document picker. These files stay on this phone; website sign-out clears browser state, while saved outputs remain private to the same Allternit account.

## Developer contract

`LocalImageStore` uses `filesDir/aci-images/<SHA-256-account-id>` and atomic `turns.json` plus PNG files. Confirmation saves the captured session, request, user, assistant, and tool identities before any provider send. A new phone-only conversation receives `android-local-<UUID>`; an existing native chat keeps its ID. The PNG is saved before the completed turn index. Loading a runtime chat overlays the local tool result into its original assistant turn. Outputs reads the local index without an API. Interrupted entries restore an actionable failure, never a resend. No backend session create, upload, image note, or artifact write is on the local path.

Relayed requests acquire POST `pane-render/:id/claim` with a random `executorId` before they are offered to the person. Only a successful claim enables confirmation. Every progress and reply body includes the same executor ID, including declined, unsupported, and replay errors. A failed/conflicting claim performs no provider operation or reply. The native prompt confirmation remains separate. Local-only jobs do not call these routes.

The fixed adapter supports modern user bubbles/conversation-role containers and legacy author-role nodes, the accessible Send prompt control and legacy send button. It refuses temporary chats, existing turns, missing image-mode selection, ambiguous sends, and uncorrelated output. Transport uses `browser-image`; every native media tool input uses `kind: image`. Preview remains the bounded real thumbnail at stage `rendering`; source and PNG limits remain enforced. Shell account ownership clears active progress, cancels observation, destroys the WebView and clears website cookies/storage on teardown.

Verification in the correction pass is limited to source/call-site readback, `git diff --check`, and syntax checks of the five embedded JavaScript operations. Gradle compilation, suites and signed-in device/provider acceptance are reserved for the root/build train.
