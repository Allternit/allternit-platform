// Zoom web join errors -> the sidecar's decisions (adapters/zoom-web.ts
// webJoinFailure). Owner-absent must retry (the OBF bot waits for the user who
// connected Zoom); a bad OBF token or passcode must end the join with the
// reason the meeting page shows.
import { strict as assert } from "node:assert";
import { test } from "node:test";
import { webJoinFailure } from "../../src/adapters/zoom-web.js";

test("the authorising user not in the meeting yet is retried, not failed", () => {
  assert.deepEqual(webJoinFailure(3707, "The authorized user is not in the meeting"), { ownerAbsent: true });
  assert.deepEqual(webJoinFailure(null, "MEETING_FAIL_AUTHORIZED_USER_NOT_INMEETING"), { ownerAbsent: true });
});

test("refusals map to the contract reasons", () => {
  const obf = webJoinFailure(3712, "Invalid on behalf token");
  assert.equal(obf.ownerAbsent, false);
  assert.equal(!obf.ownerAbsent && obf.reason, "guest_blocked");
  const pwd = webJoinFailure(3004, "Wrong meeting passcode");
  assert.equal(!pwd.ownerAbsent && pwd.reason, "join_failed");
  assert.match(!pwd.ownerAbsent ? pwd.detail : "", /passcode/);
  const other = webJoinFailure(1, "Meeting does not exist");
  assert.equal(!other.ownerAbsent && other.reason, "join_failed");
});
