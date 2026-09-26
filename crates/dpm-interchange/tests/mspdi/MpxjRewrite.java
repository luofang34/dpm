import org.mpxj.*;
import org.mpxj.mspdi.MSPDIReader;
import org.mpxj.mspdi.MSPDIWriter;
import java.time.LocalDateTime;

/**
 * Reads a DPM-exported MSPDI document with MPXJ, prints what MPXJ understood, and writes MPXJ's own
 * encoding of it, so DPM can re-import another implementation's rendering of its export.
 * Usage: java -cp "<MPXJ lib directory>/*" MpxjRewrite.java <dpm-export.xml> <out.xml>
 */
public class MpxjRewrite {
    public static void main(String[] args) throws Exception {
        ProjectFile file = new MSPDIReader().read(args[0]);
        ProjectProperties props = file.getProjectProperties();
        props.setCurrentDate(LocalDateTime.of(2026, 1, 1, 0, 0));
        System.out.println("project name=" + props.getName() + " guid=" + props.getGUID());
        for (Task t : file.getTasks()) {
            StringBuilder links = new StringBuilder();
            for (Relation r : t.getPredecessors()) {
                links.append(" [").append(r.getPredecessorTask().getUniqueID()).append(' ')
                     .append(r.getType()).append(" lag=").append(r.getLag()).append(']');
            }
            System.out.println("uid=" + t.getUniqueID() + " guid=" + t.getGUID() + " level=" + t.getOutlineLevel()
                + " name=" + t.getName() + " duration=" + t.getDuration() + " milestone=" + t.getMilestone()
                + " summary=" + t.getSummary() + " priority=" + t.getPriority() + " notes=" + t.getNotes() + links);
        }
        new MSPDIWriter().write(file, args[1]);
    }
}
