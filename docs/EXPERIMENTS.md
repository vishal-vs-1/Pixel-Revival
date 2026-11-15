# Experiment history

This is the contributor record of major experiments, their results, and what happened next. [Project flow](PROJECT_FLOW.md) describes the current implementation. Detailed run notes, captures, and working files stay in the local history.

| When | What we tried | What happened | Decision |
| --- | --- | --- | --- |
| Through Nov 5, 2026 | Split game scenes into depth layers and tested parallax, capture, and motion tracking. | A hand-built preview showed parallax and some synthetic checks passed. The trials did not establish automatic conversion during play. | Parked. The local history index points to the archived designs and run records. |
| Nov 6–14, 2026 | Used the shared native graphics renderer for Java ME and NES. | A synthetic Java ME scene matched across renderers. Later Java ME gameplay reports showed low frame rates or full command queues on the shared path. Those reports apply to the sessions tested. | Java ME uses its compatibility GPU or CPU path. NES Enhanced uses the shared renderer. |
| Nov 10–12, 2026 | Compared Scale2x, alternate pixel filters, and local AI upscalers. Also tried repeatedly upscaling one small pose as far as 256×. | No alternative filter won across the stills reviewed. AI outputs could change lettering or sprite details. Repeated passes enlarged the reinterpretation of the pose. | Kept Fast Pixel and optional AI Cartoon as the general artwork choices. |
| Nov 10–12, 2026 | Enhanced NES artwork in groups, then split the results back into tiles. | A staged group reached the player, but the runs did not confirm a visual win during gameplay. Larger groups reduced calls for some filters. A comparison of 8–128 cells used one composited frame crop and does not establish sprite or animation quality. | Grouping remains an experiment. The player pose workflow uses detected sprite layouts. |
| Nov 12, 2026 | Hosted the Mesen core in a launcher owned window. | The core built and initialized in a smoke check. A game launch reached startup and later crashed during native cleanup. This route still takes a composed NES frame from Mesen. | Kept the embedded host behind explicit opt-in. The default launches managed Mesen as a separate process. |
| Nov 14–15, 2026 | Tried tracing and hand drawing a single pose, then built an offline renderer that estimates shared color boundaries from the captured pixels. | The first trace produced distracting outlines, and hand drawing only covered one pose. The automatic version added controlled ink and stronger colors. Across 51 saved poses from four games, it preserved the scaled visible bounds. The largest PNG in that run was about 5.2 KB. | Added bulk regeneration and an explicit playback option for NES and Java ME. Atlas, fitting, and headless checks passed. Live gameplay coverage remains unverified. |

## Repeatable checks

- Run `python -m runtime_assets.sprite_paint input.png output.png` on a transparent pixel sprite to try the pose renderer. See [the generated pose tests](../tests/test_generated_poses.py) for height, orientation, and storage checks.
- See [the embedded NES tests](../tests/test_nes_inprocess.py) for pack and core lifecycle fixtures and [Development](DEVELOPMENT.md#checks) for the build and test setup.

Before finishing a major task, record each major experiment and its result here. Include the date, question and setup, observed result, limits of the evidence, and decision. Link reproducible checks or a commit when available. Include failed, inconclusive, and parked attempts.
