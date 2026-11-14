// What Live heard -> a meeting turn only when the bot is addressed (live-tile.ts
// liveWake, same rules as the worker's wake.rs). A false hit makes Gizzi answer
// people talking about it; a miss makes it ignore a direct question.
import { strict as assert } from "node:assert";
import { test } from "node:test";
import { liveWake } from "../../src/live-tile.js";

test("addressed: name first, after a greeting, or after a pause, as STT spells it", () => {
  assert.deepEqual(liveWake("Hey Gizzi, what is 2-plus 2?"), { heard: "Hey Gizzi", question: "what is 2-plus 2?" });
  assert.equal(liveWake("Jizzy, take a note")?.question, "take a note");
  assert.equal(liveWake("thanks Jordan. Gizzy what did we decide?")?.question, "what did we decide?");
  assert.equal(liveWake("ok giz ee, recap please")?.question, "recap please");
});

test("the name alone waits for the question", () => {
  assert.deepEqual(liveWake("Gizzi?"), { heard: "Gizzi", question: "" });
});

test("not addressed: talk about the bot, possessives, sound-alike words", () => {
  assert.equal(liveWake("I think Gizzi is taking notes"), null);
  assert.equal(liveWake("Gizzi's notes were good"), null);
  assert.equal(liveWake("that gizmo is dizzy and jazzy"), null);
  assert.equal(liveWake("Jesse, can you share?"), null);
});
