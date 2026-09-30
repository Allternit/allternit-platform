# Vendor look packs

**What this is:** how a vendor bot's thread is styled to feel like its own platform, and the rules that keep Allternit's provenance visible.
**Who it's for:** engineers writing or reviewing a look pack.
**Last verified against:** platform commit `c3af0730ca`, allternit-ai commit `88f4d6f6`.

Code is in the web repo (`allternit-ai`): `src/lib/gateway/look-packs/`, `src/lib/gateway/look-profile.ts`, `src/components/gateway/`. A copy of this page for that repo is `docs/gateway-look-packs.md`. Related: [ARCHITECTURE.md](ARCHITECTURE.md), [ADAPTERS.md](ADAPTERS.md).

## Why

Inside a vendor bot's thread, the user should be able to tell which vendor they are using without opening Details. It must not read as Allternit's text box with a logo on top. Vendor style is not vendor authority, though, so Allternit keeps a provenance layer the pack cannot hide.

## Ownership boundary

| Allternit owns | The vendor pack owns, inside that vendor's thread |
|---|---|
| Project navigation | Transcript presentation |
| Bot / Thread hierarchy and the thread list | Composer presentation |
| Thread status and progress | Activity presentation |
| Inspector shell | Vendor-native content (cards, routines, artifacts, tool blocks) |
| Artifacts library | Vendor terminology (`statusVocabulary`) |
| Memory | Vendor interaction patterns |
| Approvals and governance shell | Approval card face, task and routine cards, computer view |
| Connection, auth, provenance, observability | Avatar and bot-card treatment |

The boundary is enforced in types. `PackProps<P>` forbids the props `nav, navigation, threadList, inspector, artifacts, artifactsLibrary, memory, provenance, provenanceBar`, so a pack component cannot be handed any of them. `lookCssVars` emits only namespaced `--look-accent-*` and `--look-type-*` variables for the pack's own container.

## Mandatory provenance layer

`GatewayProvenanceBar` (`src/components/gateway/GatewayProvenanceBar.tsx`) renders above the pack surface. It shows agent name, vendor plus Verified, mode, lane, guarantee (when not `exact`), and Allternit status and progress. The model comes from `buildProvenance` in `src/lib/gateway/provenance.ts`, which takes no `LookProfile` input. `LookProfile` has no field that can suppress the layer, and the contract test `look profile has no provenance-suppression field and strips smuggled ones` (`FC/gateway.test.ts`) checks the shared schema.

Verified is shown only after the verification probe passes (`isVerified` in `wizard.ts`, from account `state` and `verifiedAt`). It is never decorative.

## LookProfile and LookPack

`LookProfile` (`look-profile.ts`) is data: `vendorId, avatarTreatment, accentTokens, typographyHints, iconAssets, threadHeader, activityView, statusVocabulary, approvalCard, computerView, taskCard, routineCard, composer, contentRenderers, viaBadge, verifiedBadge, laneBadge`. Adapters ship the data as `services/subscription-gateway/adapters/<id>/look-profile.json`.

`LookPack` (`look-packs/registry.ts`) is the profile plus React components:

| Field | Required | Surface |
|---|---|---|
| `Transcript`, `MessageRow`, `Composer` | yes | Thread transcript and composer |
| `ActivityView` | no | Activity |
| `ApprovalCardFace` | no | Face of the vendor-authority approval card. The approval logic stays Allternit's. |
| `TaskCard`, `RoutineCard` | no | Tasks and routines |
| `ComputerView` | no | Computer frames |
| `contentRenderers` | yes | Map from vendor content type to renderer. `'artifact'` covers `agent.artifact.created`. |

Registry: `registerLookPack`, `unregisterLookPack`, `getLookPack`, `hasLookPack`. Built-in packs are registered once at app start from `look-packs/builtin.ts` (imported in `src/main.tsx`).

Vendor artifacts render in the pack's style and are also registered into Allternit's artifacts library exactly once.

## How to add a pack

1. Add the adapter's `look-profile.json` and its assets ([ADAPTERS.md](ADAPTERS.md)).
2. Create `src/lib/gateway/look-packs/<vendor>/index.tsx` exporting a `LookPack`. Copy the shape of `grok-bot/` or `claude/`. `reference.tsx` is a deliberately minimal pack (transcript and composer only) used to prove that every other surface records a PackGap.
3. Register it in `builtin.ts`. Register one pack under every vendor id that maps to it (the Grok pack is registered as `grok-bot` and `xai`, the Claude pack as `claude-managed-agents`, `anthropic`, `claude-desktop`).
4. Add a vendor seed to `VENDOR_PACKS` in `src/lib/gateway/vendor-packs.ts` (display name, connection profiles, terms warning). Connection truth still comes from the API.
5. Write a `<vendor>.test.tsx` and extend the acceptance checklist below.

Do not add props for host surfaces, and do not reach into the provenance bar.

## PackGap contract

When a vendor sends an object the pack cannot render, the pack shows a generic fallback card (`PackFallbackCard`) and the gap is reported. Engineering does not leave raw JSON in the transcript and call the pack complete.

```
PackGap { vendor, capability, surface, fallbackUsed, severity, status, firstSeenAt, lastSeenAt, occurrences, sampleRef? }
surface:  transcript | composer | activity | card | computer | approval
severity: visual_parity | functional | data_loss
status:   open | resolved | wontfix
```

- Client: `gapsFor(pack, items, activity)` finds gaps in the current transcript. `classifyGap`: `data_loss` if the content cannot be shown, `functional` if it is an approval or interactive (has `actions` or `options`, or `interactive: true`), otherwise `visual_parity`. `createGapReporter` sends each gap key at most once per vendor and session.
- Server: `POST /gateway/vendor-packs/:vendor/gaps` upserts into `vendor_pack_gaps` (repeat reports bump `occurrences` and `last_seen_at`). `PATCH /gateway/vendor-pack-gaps/:id` changes `status` or `severity`. See [AAI_REST.md](AAI_REST.md#vendor-packs).

### Parity rules

`GET /gateway/vendor-packs/:vendor/parity` returns `{vendor, parity, openGaps, blockingGaps}`, computed in `agent_gateway_routes.rs`:

| Parity | Condition |
|---|---|
| `full` | No open gaps |
| `partial` | Every open gap is `visual_parity` |
| `blocked` | Any open `data_loss` or `functional` gap |

The library card shows this as a chip (`ParityChip` in `GatewayLibrary.tsx`: Full parity, Partial parity, Blocked). A pack ships at partial parity only if the owner accepts the visual gaps. A `data_loss` gap blocks shipping.

## Acceptance checklist

The web test `look-packs/look-pack-acceptance.test.tsx` covers the automated items. Run the last two by hand for each new pack.

- [ ] Vendor, agent and connection are identifiable without opening Details.
- [ ] With the provenance layer removed, the pack surface alone still names the vendor.
- [ ] Transcript and composer are the pack's components, not Allternit's.
- [ ] The pack's status vocabulary and terminology come from its profile.
- [ ] A `VendorMark` is present inside the pack surface (header, avatar and composer; required since ai #282).
- [ ] Allternit status, progress and provenance stay visible and unchanged around the pack.
- [ ] Vendor artifacts are registered into the artifacts library exactly once.
- [ ] An unsupported vendor object records a PackGap and is never silent plain text.
- [ ] Both light and dark themes render, with no tan Allternit surfaces leaking in.
- [ ] `GET /gateway/vendor-packs/<vendor>/parity` is `full`, or `partial` with the owner's sign-off, and never `blocked`.

## Status

Look packs exist for two vendor families: Grok Bot (`grok-bot`, `xai`) and Claude (`claude-managed-agents`, `anthropic`, `claude-desktop`). dots (OpenAI), OpenClaw and Muse have vendor library seeds and adapter `look-profile.json` data where an adapter exists, but no registered React pack in `builtin.ts` at this commit. Threads bound to those vendors render without a vendor pack until one is written. Real vendor traffic has not been used to exercise pack gaps except for the Grok Bot live run.
