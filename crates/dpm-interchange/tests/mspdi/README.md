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
