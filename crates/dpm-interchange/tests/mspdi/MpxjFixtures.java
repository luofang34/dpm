import org.mpxj.*;
import org.mpxj.mspdi.MSPDIWriter;
import java.time.LocalDateTime;
import java.util.UUID;

/**
 * Generates MSPDI fixtures for DPM's interchange tests with MPXJ's MSPDIWriter.
 * Usage: java -cp "<MPXJ lib directory>/*" MpxjFixtures.java <out-dir>
 */
public class MpxjFixtures {
    static int guidCounter = 0;

    static UUID guid(String prefix) {
        guidCounter++;
        return UUID.fromString(String.format("%s-0000-4000-8000-%012d", prefix, guidCounter));
    }

    static Task task(Task parent, ProjectFile file, String name, Duration duration, String guidPrefix) {
        Task t = parent == null ? file.addTask() : parent.addTask();
        t.setName(name);
        if (duration != null) t.setDuration(duration);
        if (guidPrefix != null) t.setGUID(guid(guidPrefix));
        return t;
    }

    static void link(Task pred, Task succ, RelationType type, Duration lag) {
        succ.addPredecessor(new Relation.Builder().predecessorTask(pred).type(type).lag(lag));
    }

    static ProjectFile base(String title, String guid) {
        ProjectFile file = new ProjectFile();
        ProjectProperties props = file.getProjectProperties();
        props.setName(title);
        props.setProjectTitle(title);
        props.setGUID(UUID.fromString(guid));
        props.setAuthor("DPM fixture generator");
        props.setStartDate(LocalDateTime.of(2026, 1, 5, 8, 0));
        props.setCreationDate(LocalDateTime.of(2026, 1, 1, 0, 0));
        props.setLastSaved(LocalDateTime.of(2026, 1, 1, 0, 0));
        props.setCurrentDate(LocalDateTime.of(2026, 1, 1, 0, 0));
        file.addDefaultBaseCalendar();
        return file;
    }

    static ProjectFile releasePlan() {
        guidCounter = 0;
        ProjectFile file = base("Release plan", "6f1c2a4e-1b7d-4c55-9a0e-3d2f5b8c9e10");
        Task root = file.addTask();
        root.setUniqueID(Integer.valueOf(0));
        root.setID(Integer.valueOf(0));
        root.setOutlineLevel(Integer.valueOf(0));
        root.setName("Release plan");
        root.setSummary(true);
        root.setGUID(UUID.fromString("a1000000-0000-4000-8000-000000000000"));
        Task design = task(null, file, "Design", null, "a1000000");
        Task spec = task(design, file, "Write specification", Duration.getInstance(16, TimeUnit.HOURS), "a1000000");
        spec.setNotes("Specify the release scope and interfaces.");
        Task review = task(design, file, "Review specification", Duration.getInstance(1, TimeUnit.DAYS), "a1000000");
        Task approved = task(design, file, "Design approved", Duration.getInstance(0, TimeUnit.DAYS), "a1000000");
        approved.setMilestone(true);
        Task build = task(null, file, "Build", null, "a1000000");
        Task implement = task(build, file, "Implement", Duration.getInstance(40, TimeUnit.HOURS), "a1000000");
        implement.setPriority(Priority.getInstance(700));
        Task docs = task(build, file, "Write documentation", Duration.getInstance(12, TimeUnit.ELAPSED_HOURS), "a1000000");
        Task test = task(build, file, "Integration test", Duration.getInstance(2, TimeUnit.DAYS), "a1000000");
        Task release = task(null, file, "Release", Duration.getInstance(0, TimeUnit.HOURS), "a1000000");
        release.setMilestone(true);
        link(spec, review, RelationType.FINISH_START, Duration.getInstance(0, TimeUnit.HOURS));
        link(review, approved, RelationType.FINISH_START, Duration.getInstance(2, TimeUnit.ELAPSED_HOURS));
        link(design, build, RelationType.FINISH_START, Duration.getInstance(1, TimeUnit.DAYS));
        link(implement, docs, RelationType.START_START, Duration.getInstance(4, TimeUnit.HOURS));
        link(implement, test, RelationType.FINISH_FINISH, Duration.getInstance(-3, TimeUnit.HOURS));
        link(docs, release, RelationType.START_FINISH, Duration.getInstance(1, TimeUnit.ELAPSED_DAYS));
        link(test, release, RelationType.FINISH_START, Duration.getInstance(0, TimeUnit.HOURS));
        return file;
    }

    static ProjectFile unsupported() {
        guidCounter = 0;
        ProjectFile file = base("Scheduling features outside the DPM subset", "0b9d7c6e-2f41-4a8b-b3c5-7e6f1d2a4b30");
        Resource engineer = file.addResource();
        engineer.setName("Engineer");
        Task phase = task(null, file, "Phase", null, "b2000000");
        Task constrained = task(phase, file, "Constrained task", Duration.getInstance(8, TimeUnit.HOURS), "b2000000");
        constrained.setConstraintType(ConstraintType.START_NO_EARLIER_THAN);
        constrained.setConstraintDate(LocalDateTime.of(2026, 2, 2, 8, 0));
        constrained.setDeadline(LocalDateTime.of(2026, 2, 20, 17, 0));
        Task assigned = task(phase, file, "Assigned task", Duration.getInstance(3, TimeUnit.DAYS), "b2000000");
        assigned.addResourceAssignment(engineer);
        assigned.setPercentageComplete(Integer.valueOf(50));
        Task finished = task(phase, file, "Finished task", Duration.getInstance(4, TimeUnit.HOURS), "b2000000");
        finished.setPercentageComplete(Integer.valueOf(100));
        finished.setActualStart(LocalDateTime.of(2026, 1, 5, 8, 0));
        finished.setActualFinish(LocalDateTime.of(2026, 1, 5, 12, 0));
        Task baselined = task(phase, file, "Baselined task", Duration.getInstance(6, TimeUnit.HOURS), "b2000000");
        baselined.setBaselineDuration(Duration.getInstance(5, TimeUnit.HOURS));
        baselined.setText(1, "custom field value");
        Task percentLag = task(phase, file, "Percent lag successor", Duration.getInstance(2, TimeUnit.HOURS), "b2000000");
        Task zero = task(phase, file, "Zero duration task", Duration.getInstance(0, TimeUnit.HOURS), "b2000000");
        Task longMilestone = task(phase, file, "Milestone with duration", Duration.getInstance(1, TimeUnit.DAYS), "b2000000");
        longMilestone.setMilestone(true);
        Task noGuid = task(phase, file, "Task without GUID", Duration.getInstance(1, TimeUnit.HOURS), null);
        Task inactive = task(phase, file, "Inactive task", Duration.getInstance(1, TimeUnit.HOURS), "b2000000");
        inactive.setActive(false);
        Task follow = task(null, file, "Follow-up", Duration.getInstance(2, TimeUnit.HOURS), "b2000000");
        link(constrained, assigned, RelationType.FINISH_START, Duration.getInstance(0, TimeUnit.HOURS));
        link(constrained, percentLag, RelationType.FINISH_START, Duration.getInstance(50, TimeUnit.PERCENT));
        link(phase, follow, RelationType.START_START, Duration.getInstance(0, TimeUnit.HOURS));
        link(noGuid, follow, RelationType.FINISH_START, Duration.getInstance(1, TimeUnit.WEEKS));
        return file;
    }

    public static void main(String[] args) throws Exception {
        String out = args[0];
        new MSPDIWriter().write(releasePlan(), out + "/mpxj-release-plan.xml");
        new MSPDIWriter().write(unsupported(), out + "/mpxj-unsupported-features.xml");
    }
}
