# MSPDI fixtures

The `mpxj-*.xml` documents were written by MPXJ's `MSPDIWriter` (MPXJ 16.9.0, from the `mpxj`
Python package's bundled jars, run under OpenJDK 25), not by DPM, so import tests read another
implementation's encoding of durations, lag formats, outline levels and unsupported data. MPXJ is
LGPL-licensed; it ran as a separate process to produce the files and is neither linked into nor
shipped with DPM. The project content is synthetic and authored for these tests in
[MpxjFixtures.java](MpxjFixtures.java), which regenerates byte-identical files:

```sh
java -cp "<MPXJ lib directory>/*" MpxjFixtures.java <out-dir>
```

These files are not Microsoft Project output. MPXJ follows the published MSPDI schema; files
saved by Microsoft Project itself may carry elements these fixtures lack, which the importer
reports rather than drops.

- `mpxj-release-plan.xml`: the supported subset with a project summary task, nested summaries,
  milestones, notes, priority, working and elapsed durations, all four relation types, working
  and elapsed lags, a negative lag and a summary-to-summary link.
- `mpxj-unsupported-features.xml`: a date constraint and deadline, a resource assignment, partial
  and complete progress with actuals, a baseline and custom field, a percentage lag, an SS link
  from a summary, a zero-duration task, a milestone with a duration, a task without a GUID and an
  inactive task.
- `dpm-export-release-plan.xml`: DPM's own export of the imported release plan. It pins the
  export format and is not an external fixture.
- `mpxj-rewrite-of-dpm-export.xml`: MPXJ's `MSPDIReader` reading `dpm-export-release-plan.xml`
  and its `MSPDIWriter` writing it again ([MpxjRewrite.java](MpxjRewrite.java)), so DPM's export
  is checked against another implementation's reading of it.

## OmniPlan fixtures

The `omniplan-*.xml` documents were saved by OmniPlan 4.10.3 (Mac App Store build,
`com.omnigroup.OmniPlan4.MacAppStore`) on the maintainer's machine through Omni Automation's
`makeFileWrapper(name, "com.microsoft.project.mspdi")`, and are committed byte for byte as OmniPlan
wrote them. Their content is ours and synthetic; OmniPlan only encoded it. They carry no GUIDs,
no `LagFormat`, no `DurationFormat` and the project name only as `<Title>`, which is why the
importer needs an explicit key prefix for them.

- `omniplan-native.xml`: a new OmniPlan document authored by script, not a conversion.
  [OmniPlanNative.js](OmniPlanNative.js) (run with AppleScript `evaluate javascript`) creates two
  groups with CJK-named tasks, two milestones, priorities, a note, a resource with one assignment,
  a locked start date and FS/SS/FF/SF dependencies between normal tasks.
  [OmniPlanNative.applescript](OmniPlanNative.applescript) then sets what Omni Automation does not
  expose: positive and negative lead times and start-after / end-before constraint dates. OmniPlan
  writes the locked start as `ConstraintType` 2 (Must Start On) and omits the start-after and
  end-before constraints from MSPDI entirely; SF between two normal tasks is written as `Type` 2;
  its 0..9 priority is written as ⌊n·1000/9⌋ (3→333, 5→555, 7→777, 9→1000, unset 0 for groups and
  milestones). Re-running the scripts reproduces the same outline, kinds, priorities, relations and
  leads; the dates in the file depend on the day it is run.
- `dpm-export-cjk.xml`: DPM's export of `tests/support/execution-plan.json` after importing the
  MPXJ release plan with two task names in Chinese; the file OmniPlan opened. It pins the pre-image for the round
  trip and is not an external fixture.
- `omniplan-export-of-dpm.xml`: OmniPlan's MSPDI export after opening `dpm-export-cjk.xml`. OmniPlan
  dropped every GUID, wrote priority 0 for groups, added a line break to every note, and rewrote
  the SF link into the zero-duration Release milestone as SS (the same bound for a milestone).
