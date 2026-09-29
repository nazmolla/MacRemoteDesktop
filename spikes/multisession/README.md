# Multi-session spike (throwaway)
Question (spec §5.3): how do Apple Screen Sharing and Jump Desktop run simultaneous
remote sessions for different users, and can our session agent create virtual
displays, capture, and inject input inside a background (non-console) session?
Nothing here ships. The deliverable is docs/research/2026-09-29-multisession-spike.md.

`probe/` — prints one JSON line describing what works from the session it runs in
(virtual display, ScreenCaptureKit capture, CGEvent input). Build with `probe/build.sh`.
