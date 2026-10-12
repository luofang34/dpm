// What indenting and outdenting propose: a parent change of the work, never a move of its dates. Indent puts it under
// the work package listed just above it at the same level; outdent puts it beside its parent. The application decides
// whether the change may be made: protected work and its ancestors and prerequisites are refused there.

import Foundation

extension ObserverModel {
    /// The parent change an indent of `identity` proposes, or nil when no work package is listed above it at its level.
    public func indentEdit(of identity: String) -> PlanEdit? {
        let parent = draftParent(of: identity)
        // Listed as the Gantt draws the rows when it draws this one, otherwise in the plan's order (a row folded away or
        // filtered out is still indented from the inspector).
        let drawn = outline
        let listed = drawn.contains(where: { $0.id == identity }) ? drawn.map { ($0.id, $0.row.kind) } : snapshot.inventory.items.map { ($0.identity, $0.kind) }
        let siblings = listed.filter { draftParent(of: $0.0) == parent }
        guard let at = siblings.firstIndex(where: { $0.0 == identity }),
              let package = siblings[..<at].last(where: { $0.1 == "WorkPackage" }) else { return nil }
        return .parent(work: identity, parent: package.0)
    }

    /// The parent change an outdent of `identity` proposes, or nil at the top level.
    public func outdentEdit(of identity: String) -> PlanEdit? {
        guard let parent = draftParent(of: identity) else { return nil }
        return .parent(work: identity, parent: draftParent(of: parent))
    }

    /// The parent of `identity` once the draft applies: a parent change already drafted counts, so Shift-Tab after Tab
    /// takes the row back out and a second Tab nests it deeper.
    func draftParent(of identity: String) -> String? {
        for edit in (editing.draft?.edits ?? []).reversed() {
            if case let .parent(work, parent) = edit, work == identity { return parent }
        }
        return editParent(of: identity)
    }

    /// Propose the indent or outdent of the cursor row, or say why there is none.
    func nest(_ indent: Bool) {
        guard let row = focusedRow else { return }
        if let proposal = indent ? indentEdit(of: row.id) : outdentEdit(of: row.id) {
            edit(proposal)
        } else {
            setEditNotice(indent ? "No work package is drawn above \(row.row.key) at its level to indent it into."
                                 : "\(row.row.key) is already at the top level.")
        }
    }
}
