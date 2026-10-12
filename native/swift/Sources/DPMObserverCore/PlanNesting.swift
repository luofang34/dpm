// What indenting and outdenting propose: a parent change of the work, never a move of its dates. Indent puts it under
// the work package listed just above it at the same level; outdent puts it beside its parent. The application decides
// whether the change may be made: protected work and its ancestors and prerequisites are refused there.

import Foundation

extension ObserverModel {
    /// The parent change an indent of `identity` proposes, or nil when no work package is listed above it at its level.
    public func indentEdit(of identity: String) -> PlanEdit? {
        let parent = editParent(of: identity)
        // Listed as the Gantt shows the rows when it has them, otherwise in the plan's order.
        let listed = snapshot.gantt != nil ? outline.map { ($0.id, $0.row.kind) } : snapshot.inventory.items.map { ($0.identity, $0.kind) }
        let siblings = listed.filter { editParent(of: $0.0) == parent }
        guard let at = siblings.firstIndex(where: { $0.0 == identity }),
              let package = siblings[..<at].last(where: { $0.1 == "WorkPackage" }) else { return nil }
        return .parent(work: identity, parent: package.0)
    }

    /// The parent change an outdent of `identity` proposes, or nil at the top level.
    public func outdentEdit(of identity: String) -> PlanEdit? {
        guard let parent = editParent(of: identity) else { return nil }
        return .parent(work: identity, parent: editParent(of: parent))
    }

    /// Propose the indent or outdent of the cursor row, or say why there is none.
    func nest(_ indent: Bool) {
        guard let row = focusedRow else { return }
        if let proposal = indent ? indentEdit(of: row.id) : outdentEdit(of: row.id) {
            edit(proposal)
        } else {
            setEditNotice(indent ? "No work package is listed just above \(row.row.key) at its level to indent it into."
                                 : "\(row.row.key) is already at the top level.")
        }
    }
}
