---
status: later
blocked-by: [T-065]
---
# T-066: Plugin host: manifests, processes and the protocol

Find plugin folders after the first frame, read their manifests, and run
enabled plugins as background or on-demand processes that get events and
return actions as JSON lines over stdio, with the bounds, the exit and
crash handling and the quit behavior of spec 0016 (flows 1, 3, 4, 6, 7).
Includes the `plugins:` config block with its template in
`src/default-config.yaml`, and the `--bench-startup` comparison with three
background plugins enabled. Waits for spec 0016's approval. Area: src.

Spec: ../spec/0016-plugin-system.md
