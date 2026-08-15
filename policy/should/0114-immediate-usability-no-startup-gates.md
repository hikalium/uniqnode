# 0114 Immediate usability — no startup gates in front of the main UI

Level: SHOULD
Scope: any agent building an interactive deliverable (web page, TUI, interactive CLI) — and
anything reviewing or scoring such deliverables.

The deliverable's primary interface SHOULD be visible and operable the moment it opens. Do not
place a gate screen — a "Start" button, a splash page, a welcome interstitial — between the user
and the main UI.

When a platform restriction genuinely requires a user gesture (e.g. the browser autoplay policy
for AudioContext, permission prompts), attach the unlock to the first natural interaction with the
already-visible UI (the first click or keypress on a piano key resumes the AudioContext), not to a
dedicated blocking button.

Acceptable exceptions: legally or safety-required confirmations, guards before destructive
actions, credential entry.

Rationale: measured 2026-07-08 (pianoapp): a "Start Piano" gate hid the keyboard until clicked.
The autoplay policy only requires a gesture before sound, not before showing the UI — the gate
added a step for every user, contradicted the task's own completion criterion ("the UI is done
once the keyboard renders"), and degraded automated verification (a uitest at load sees the gate,
not the app).

Enforcement: convention, plus verification-side visibility — a load-time uitest should find the
primary UI element (contains-check); reviewers treat a startup gate as a UX defect unless it maps
to a listed exception.
