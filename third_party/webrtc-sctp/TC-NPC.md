# SCTP dependency for mistlib

This source and its licenses come from `third_party/webrtc-sctp` in
mistlib tag `v0.7.3-beta.6` (commit `1ec92952ca7b674ef78fd3241bb151017876e747`).
The Cargo patch lets mistlib-native and WebRTC share the same crate,
including mistlib's receive buffer configuration hook. Cargo resolves
patches before selecting build features, so the patch uses a local path.

When updating mistlib, check its SCTP patch and refresh this copy to match.
The documentation image and fuzz-test fixtures live in the upstream tree.
[mistlib tag](https://github.com/tik-choco-lab/mistlib/tree/v0.7.3-beta.6/third_party/webrtc-sctp)
