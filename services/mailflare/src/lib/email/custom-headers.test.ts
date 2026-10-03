import { test } from "node:test";
import assert from "node:assert/strict";
import {
	ALLOWED_OUTBOUND_HEADERS,
	compactOutboundHeaders,
	normalizeOutboundHeaders,
} from "./custom-headers.ts";

test("allows exactly the four sanctioned headers", () => {
	const result = normalizeOutboundHeaders({
		"In-Reply-To": "<in-1@acme.com>",
		References: "<a@x> <b@x>",
		"Auto-Submitted": "auto-replied",
		"List-Unsubscribe": "<https://x.test/u>",
	});
	assert.deepEqual(result, {
		ok: true,
		headers: {
			"In-Reply-To": "<in-1@acme.com>",
			References: "<a@x> <b@x>",
			"Auto-Submitted": "auto-replied",
			"List-Unsubscribe": "<https://x.test/u>",
		},
	});
});

test("rejects any header outside the allow-list", () => {
	const result = normalizeOutboundHeaders({ "X-Evil": "1", Subject: "nope" });
	assert.equal(result.ok, false);
	if (!result.ok) assert.deepEqual(result.rejected.sort(), ["Subject", "X-Evil"]);
});

test("rejects mixed allowed and disallowed", () => {
	const result = normalizeOutboundHeaders({ "Auto-Submitted": "auto-replied", Cc: "x@y.z" });
	assert.equal(result.ok, false);
});

test("matches case-insensitively and canonicalizes", () => {
	const result = normalizeOutboundHeaders({ "in-reply-to": "<m@x>", "auto-submitted": "auto-replied" });
	assert.deepEqual(result, {
		ok: true,
		headers: { "In-Reply-To": "<m@x>", "Auto-Submitted": "auto-replied" },
	});
});

test("rejects duplicate headers that differ only in case", () => {
	const result = normalizeOutboundHeaders({ "In-Reply-To": "<a@x>", "in-reply-to": "<b@x>" });
	assert.equal(result.ok, false);
});

test("rejects empty and non-string values", () => {
	assert.equal(normalizeOutboundHeaders({ References: "" }).ok, false);
	assert.equal(normalizeOutboundHeaders({ References: 42 }).ok, false);
});

test("accepts missing headers and empty object", () => {
	assert.deepEqual(normalizeOutboundHeaders(undefined), { ok: true, headers: {} });
	assert.deepEqual(normalizeOutboundHeaders({}), { ok: true, headers: {} });
});

test("rejects arrays and non-object input", () => {
	assert.equal(normalizeOutboundHeaders(["In-Reply-To"]).ok, false);
	assert.equal(normalizeOutboundHeaders("In-Reply-To").ok, false);
});

test("the allow-list is exactly what the task specifies", () => {
	assert.deepEqual([...ALLOWED_OUTBOUND_HEADERS].sort(), [
		"Auto-Submitted",
		"In-Reply-To",
		"List-Unsubscribe",
		"References",
	]);
});

test("compactOutboundHeaders drops undefined and empty objects", () => {
	assert.equal(compactOutboundHeaders(undefined), undefined);
	assert.equal(compactOutboundHeaders({}), undefined);
	assert.deepEqual(
		compactOutboundHeaders({ "In-Reply-To": "<a@x>", References: undefined }),
		{ "In-Reply-To": "<a@x>" },
	);
});
