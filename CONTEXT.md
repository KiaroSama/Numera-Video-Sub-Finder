# Subtitle Extraction

A local workflow for turning visible subtitle appearances in a video into editable, timed text.

## Language

**Hardcoded subtitle**:
Text rendered into the video pixels, distinct from an embedded subtitle track or spoken audio.
_Avoid_: Transcript, soft subtitle

**Appearance**:
One continuous occurrence of a subtitle in a selected region. Identical text appearing after a gap is a new appearance.
_Avoid_: Unique string

**Region**:
A selected area of the displayed video containing subtitle candidates, with optional exclusions.
_Avoid_: Crop when describing the selected search area

**Crop**:
An image of a candidate appearance retained as visual recognition evidence.
_Avoid_: Region when describing a saved image

**Observation**:
Recognized text and available confidence from a named model at an evidenced source time.
_Avoid_: Cue before reconciliation

**Cue**:
Editable subtitle text with a validated start/end interval, track, and source evidence.
_Avoid_: Frame, transcript

**Track**:
A sequence of spatially related subtitle appearances, preserving simultaneous captions separately.
_Avoid_: Language when referring to a spatial group

**Qualification**:
Observed evidence for a target, package, language, or acceleration path; build success alone is not functional or hardware qualification.
_Avoid_: Supported without naming which evidence exists

**Update offer**:
A newer official application release presented for approval; discovery is not permission to download or install it.
_Avoid_: Automatic installation when describing a notification

**Managed runtime pack**:
A verified application-local dependency collection that can be repaired without replacing another program's tools.
_Avoid_: System installation

**Language pack**:
An identified recognition resource for a catalog language; availability is distinct from measured subtitle accuracy.
_Avoid_: Accuracy-qualified language when only the pack exists

**Development prerelease**:
Published experimental native builds with explicit remaining qualification limits, not a stable release or certified hardware support.
_Avoid_: Qualified release
