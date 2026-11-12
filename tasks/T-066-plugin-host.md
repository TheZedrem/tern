---
status: later
blocked-by: []
---
# T-066: Plugin host: manifests, processes and the protocol

Find plugin folders after the first frame, read their manifests, and run
enabled plugins as background or on-demand processes that get events and
return actions as JSON lines over stdio, with the bounds, the exit and
crash handling and the quit behavior of spec 0016 (flows 1, 3, 4, 6, 7).
Includes the `plugins:` config block with its template in
`src/default-config.yaml`, and the `--bench-startup` comparison with three
background plugins enabled. Builds threat model 0005's TM-plug-01
(discovery check), -10, -12, -13, -15, -16 (event contents), -17 and
-19, and checks -14 (only three pipes inherited) on Linux and Windows.
The kill of a plugin's process tree (TM-plug-11) follows OQ-035; until
then, `// TODO(OQ-035)` at the kill. Area: src.

Spec: ../spec/0016-plugin-system.md
