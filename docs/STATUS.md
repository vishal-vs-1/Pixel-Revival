# Current status

Source snapshot: **26 November 2026**. This page separates implemented behavior from acceptance. Earlier preview packages and historical design checklists may differ.

| Area | Implemented | Evidence or remaining limit |
|---|---|---|
| Java ME playback | Patched FreeJ2ME GPU compatibility and CPU modes | Game/driver compatibility and audible quality require ordinary gameplay acceptance |
| NES playback | In-process Rust core, audio, bounded display worker | Short focused checks passed; smoothness on every game is not established |
| General artwork | Shared persistent jobs, three styles, per-recipe cache | Missing art falls back to original; AI changes details |
| Capture and recovery | Standalone capture; Enhanced-only comparison capture; direct DB updates; manual Sync | Capture/event work still has cost; missed events require reconciliation |
| Java ME character art | Automatic candidates, manual confirmation, uploaded/generated choices | Coverage depends on eligible draw hooks and confirmed poses |
| NES character art | Fetched native bytes, layered matching, known-pose tracking and default-image fitting | Partial intended-player replacement reviewed in 15 sampled games; not complete animation or all-game support |
| Comparison | Shared controls, private saves, fullscreen handling | Java ME clocks are independent; portrait sizing still needs user acceptance |
| Preparation scheduling | Pre-play Java ME work, memory/pixel-based batches, interrupted-job recovery | Device/driver availability changes throughput; no fixed batch-size or latency guarantee |

## Open work

1. **NES initial identity:** Snow Brothers can select enemies. Kung Fu and Jackie Chan still produced no accepted poses in attempted sequences. Fix generic detection without game-title branches.
2. **Coverage:** resolve other zero-result/menu/paused cases. All 131 ROMs were attempted, but 65 nonzero candidate sets are not 65 working games. Three controller types were rejected. No valid all-games success percentage is available.
3. **Gameplay acceptance:** check running/jumping image coverage, unchanged enemies, smoothness and normal close/import after restart. Native framebuffer equivalence and focused checks do not settle these.
4. **Java ME acceptance:** verify comparison sizing, resumed batches and captured/confirmed pose behavior through the normal launcher.
5. **Build/package consistency:** shared graphics staging remains in build/package tooling even though active Java ME and NES no longer share that renderer. Audit dependencies before removing them; rebuild a preview before claiming it contains current fixes.

## Evidence boundary

The latest NES hook work passed 65 focused Python/backend checks and two Rust collector checks. Six games retained identical native framebuffer hashes before/after enabling the observer. Independent future-input checks showed partial intended-player replacement in 15 of 16 selected scenes across Original/2x/4x; Snow Brothers failed visual acceptance. This is a selected sample and screenshot review, not a compatibility rate or proof that every changed frame targeted the player.

No new runtime validation was run during the documentation cleanup. Full experiments, including failed trials, remain in [EXPERIMENTS.md](EXPERIMENTS.md); detailed local evidence is indexed in `local-work/README.md` when available.
