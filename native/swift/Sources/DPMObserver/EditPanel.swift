// Editing the plan from the Gantt and the network: the toggle that offers it, the inspector that edits the
// selected item by keyboard, the draft of edits, and the review sheet where the application's diff is read and
// the draft is applied with a reason. Every check is the application's; this only gathers and shows.


import DPMObserverCore
import SwiftUI

/// Offers editing gestures and the edit inspector; off by default.
struct EditToggle: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        Toggle(isOn: Binding(get: { model.editing.enabled }, set: { model.setEditing($0) })) {
            Label(model.editing.draft.map { "Edit (\($0.edits.count))" } ?? "Edit", systemImage: "pencil")
        }
        .toggleStyle(.button)
        .keyboardShortcut("e", modifiers: .command)
        .help("Edit the plan: drag between bars or nodes to link them, drag a bar's finish to change its estimate, or use the inspector. Edits gather in a draft and apply only after review (⌘E).")
        .accessibilityIdentifier("edit.toggle")
    }
}

/// The inspector: the selected item's fields, its links and gates, a new task, and the draft.
struct EditPanel: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let reason = model.editUnavailable {
                    Label(reason, systemImage: "lock").font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
                if let notice = model.editing.notice {
                    Label(notice, systemImage: "info.circle").font(.caption).fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("edit.notice")
                }
                LabeledContent("Acting as") {
                    TextField("kind:name", text: Binding(get: { model.editing.actor }, set: { model.setEditActor($0) }))
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("edit.actor")
                }
                .help("Attribution recorded with the change, as kind:name (human or service); not an authenticated identity.")
                if case .work(let identity)? = model.selection, let item = model.snapshot.inventory.byIdentity[identity] {
                    ItemEditor(model: model, item: item).id(identity)
                } else {
                    Text("Select a task, milestone or package to edit it.").font(.callout).foregroundStyle(.secondary)
                }
                Divider()
                NewTaskEditor(model: model)
                Divider()
                DraftList(model: model)
            }
            .padding(12)
        }
        .frame(minWidth: 280, idealWidth: 320)
        .accessibilityIdentifier("edit.panel")
    }
}

/// The selected item's own fields and its relations, each change entering the draft.
struct ItemEditor: View {
    @ObservedObject var model: ObserverModel
    let item: WorkItem
    @State private var title = ""
    @State private var objective = ""
    @State private var acceptance = ""
    @State private var optimistic = ""
    @State private var likely = ""
    @State private var pessimistic = ""
    @State private var linkKey = ""
    @State private var linkKind = "FinishStart"
    @State private var linkLag = "0"
    @State private var linkAsPredecessor = true
    @State private var gateKey = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("\(item.key) · \(item.kind == "WorkPackage" ? "work package" : item.kind.lowercased()) · \(item.status)").font(.headline)
            field("Title", $title) { model.edit(.title(work: item.identity, text: title)) }
            field("Objective", $objective, axis: .vertical) { model.edit(.objective(work: item.identity, text: objective)) }
            VStack(alignment: .leading, spacing: 4) {
                Text("Acceptance criteria, one per line").font(.caption).foregroundStyle(.secondary)
                TextField("Criteria", text: $acceptance, axis: .vertical).lineLimit(3...8).textFieldStyle(.roundedBorder)
                    .accessibilityLabel("Acceptance criteria, one per line").accessibilityIdentifier("edit.acceptance")
                Button("Set criteria") {
                    model.edit(.acceptance(work: item.identity, criteria: acceptance.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }))
                }
            }
            if item.kind == "Task" {
                HStack {
                    hours("Optimistic", $optimistic)
                    hours("Likely", $likely)
                    hours("Pessimistic", $pessimistic)
                }
                Button("Set estimate") {
                    if let o = Double(optimistic), let m = Double(likely), let p = Double(pessimistic) {
                        model.edit(.estimate(work: item.identity, optimistic: o, likely: m, pessimistic: p))
                    }
                }
                .accessibilityIdentifier("edit.estimate")
            }
            HStack {
                Picker("Kind", selection: Binding(get: { draftedKind }, set: { model.edit(.kind(work: item.identity, kind: $0)) })) {
                    Text("Task").tag("Task")
                    Text("Milestone").tag("Milestone")
                    Text("Work package").tag("WorkPackage")
                }
                .accessibilityIdentifier("edit.kind")
            }
            HStack {
                Button("Outdent") { model.edit(.parent(work: item.identity, parent: model.editParent(of: model.editParent(of: item.identity) ?? ""))) }
                    .keyboardShortcut("[", modifiers: .command)
                    .disabled(model.editParent(of: item.identity) == nil)
                    .help("Move out of its package (⌘[)")
                Button("Indent") { if let sibling = previousSibling() { model.edit(.parent(work: item.identity, parent: sibling)) } }
                    .keyboardShortcut("]", modifiers: .command)
                    .disabled(previousSibling() == nil)
                    .help("Move under the work package above it (⌘])")
            }
            Divider()
            Text("Dependencies").font(.subheadline.weight(.semibold))
            ForEach(model.snapshot.inventory.relations(of: item.identity), id: \.id) { relation in
                HStack {
                    Text("\(relation.abbreviation) \(model.editName(relation.predecessor)) → \(model.editName(relation.successor))\(relation.lagHours == 0 ? "" : " " + GanttRelation.lagWords(relation.lagHours, basis: relation.lagBasis))")
                        .font(.caption)
                    Spacer()
                    Button("Remove") { model.edit(.unlink(dependency: relation.id)) }
                        .accessibilityLabel("Remove \(relation.abbreviation) \(model.editName(relation.predecessor)) to \(model.editName(relation.successor))")
                }
            }
            HStack {
                Picker("", selection: $linkAsPredecessor) {
                    Text("after").tag(true)
                    Text("before").tag(false)
                }
                .labelsHidden().frame(width: 80)
                .accessibilityLabel("Link direction")
                .help("after KEY: KEY is the predecessor; before KEY: KEY is the successor")
                TextField("key", text: $linkKey).textFieldStyle(.roundedBorder).accessibilityLabel("Other work key").accessibilityIdentifier("edit.link-key")
            }
            HStack {
                Picker("", selection: $linkKind) {
                    ForEach(["FinishStart", "StartStart", "FinishFinish", "StartFinish"], id: \.self) { Text(GanttRelation.abbreviation($0)).tag($0) }
                }
                .labelsHidden().frame(width: 70).accessibilityLabel("Relation kind")
                TextField("lag h", text: $linkLag).textFieldStyle(.roundedBorder).frame(width: 60).accessibilityLabel("Lag in hours, negative for lead")
                Button("Add link") { addLink() }.disabled(otherIdentity == nil).accessibilityIdentifier("edit.add-link")
            }
            Divider()
            Text("Decision gates").font(.subheadline.weight(.semibold))
            ForEach(model.snapshot.inventory.decisions.filter { $0.blocks.contains(item.identity) }, id: \.identity) { decision in
                HStack {
                    Text("\(decision.key) (\(decision.status))").font(.caption)
                    Spacer()
                    Button("Remove") { model.edit(.gate(decision: decision.identity, work: item.identity, blocks: false)) }
                        .accessibilityLabel("Remove gate \(decision.key)")
                }
            }
            HStack {
                TextField("decision key", text: $gateKey).textFieldStyle(.roundedBorder).accessibilityLabel("Decision key to gate on")
                Button("Gate on") {
                    if let decision = model.snapshot.inventory.decision(key: gateKey.trimmingCharacters(in: .whitespaces)) {
                        model.edit(.gate(decision: decision.identity, work: item.identity, blocks: true))
                    }
                }
                .disabled(model.snapshot.inventory.decision(key: gateKey.trimmingCharacters(in: .whitespaces)) == nil)
            }
        }
        .onAppear(perform: load)
    }

    /// The kind the draft gives the item, so a pending change of kind shows as chosen.
    private var draftedKind: String {
        model.editing.draft?.candidate()["work_items"][item.identity]["kind"].string ?? item.kind
    }

    private var otherIdentity: String? {
        let key = linkKey.trimmingCharacters(in: .whitespaces)
        return model.snapshot.inventory.items.first { $0.key == key }?.identity
    }

    private func addLink() {
        guard let other = otherIdentity else { return }
        let ends: (PlanEnd, PlanEnd)
        switch linkKind {
        case "StartStart": ends = (.start, .start)
        case "FinishFinish": ends = (.finish, .finish)
        case "StartFinish": ends = (.start, .finish)
        default: ends = (.finish, .start)
        }
        let (from, to) = linkAsPredecessor ? (other, item.identity) : (item.identity, other)
        model.edit(.link(id: UUID().uuidString.lowercased(), from: from, fromEnd: ends.0, to: to, toEnd: ends.1, lagHours: Double(linkLag) ?? 0))
        linkKey = ""
    }

    /// The work package listed just above this item under the same parent, which an indent moves it into.
    private func previousSibling() -> String? {
        let parent = model.editParent(of: item.identity)
        let siblings = model.snapshot.inventory.items.filter { model.editParent(of: $0.identity) == parent }
        guard let at = siblings.firstIndex(where: { $0.identity == item.identity }) else { return nil }
        return siblings[..<at].last { $0.kind == "WorkPackage" }?.identity
    }

    private func load() {
        title = item.title
        objective = item.objective
        acceptance = item.acceptance.joined(separator: "\n")
        optimistic = item.estimate.map { String($0.optimisticHours) } ?? ""
        likely = item.estimate.map { String($0.likelyHours) } ?? ""
        pessimistic = item.estimate.map { String($0.pessimisticHours) } ?? ""
    }

    private func field(_ label: String, _ text: Binding<String>, axis: Axis = .horizontal, commit: @escaping () -> Void) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label).font(.caption).foregroundStyle(.secondary)
            HStack {
                TextField(label, text: text, axis: axis).lineLimit(axis == .vertical ? 2...6 : 1...1).textFieldStyle(.roundedBorder)
                    .onSubmit(commit).accessibilityLabel(label)
                Button("Set", action: commit).accessibilityLabel("Set \(label.lowercased())")
            }
        }
    }

    private func hours(_ label: String, _ text: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label).font(.caption2).foregroundStyle(.secondary)
            TextField("h", text: text).textFieldStyle(.roundedBorder).accessibilityLabel("\(label) hours")
        }
    }
}

/// A new task, entering the draft as Proposed under the selected package (or the selected item's package).
struct NewTaskEditor: View {
    @ObservedObject var model: ObserverModel
    @State private var key = ""
    @State private var title = ""
    @State private var objective = ""
    @State private var acceptance = ""
    @State private var likely = ""

    var body: some View {
        DisclosureGroup("New task") {
            VStack(alignment: .leading, spacing: 6) {
                TextField("Key, e.g. VIS-70", text: $key).textFieldStyle(.roundedBorder).accessibilityIdentifier("edit.new-key")
                TextField("Title", text: $title).textFieldStyle(.roundedBorder)
                TextField("Objective", text: $objective, axis: .vertical).lineLimit(2...4).textFieldStyle(.roundedBorder)
                TextField("Acceptance criteria, one per line", text: $acceptance, axis: .vertical).lineLimit(2...6).textFieldStyle(.roundedBorder)
                TextField("Likely hours (optimistic ½×, pessimistic 2×)", text: $likely).textFieldStyle(.roundedBorder)
                Text("Under \(parent.map(model.editName) ?? "the top level")").font(.caption).foregroundStyle(.secondary)
                Button("Add to draft") {
                    let hours = Double(likely)
                    model.edit(.newTask(NewTask(key: key.trimmingCharacters(in: .whitespaces), title: title, objective: objective,
                                                acceptance: acceptance.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty },
                                                parent: parent, estimate: hours.map { (optimistic: $0 / 2, likely: $0, pessimistic: $0 * 2) })))
                    key = ""; title = ""; objective = ""; acceptance = ""; likely = ""
                }
                .disabled(key.trimmingCharacters(in: .whitespaces).isEmpty || title.isEmpty)
                .accessibilityIdentifier("edit.new-add")
            }
            .padding(.top, 4)
        }
    }

    /// The selected package, or the package holding the selected item.
    private var parent: String? {
        guard case .work(let identity)? = model.selection, let item = model.snapshot.inventory.byIdentity[identity] else { return nil }
        return item.kind == "WorkPackage" ? identity : model.editParent(of: identity)
    }
}

/// The edits made so far, removable one by one, and the way to review them.
struct DraftList: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Draft").font(.subheadline.weight(.semibold))
            if let draft = model.editing.draft {
                Text("Against revision \(draft.basis.revision)").font(.caption).foregroundStyle(.secondary)
                ForEach(Array(draft.edits.enumerated()), id: \.offset) { index, edit in
                    HStack(alignment: .firstTextBaseline) {
                        Text(draft.words(edit, names: model.editName)).font(.caption).fixedSize(horizontal: false, vertical: true)
                        Spacer()
                        Button { model.removeEdit(at: index) } label: { Image(systemName: "xmark.circle") }
                            .buttonStyle(.borderless)
                            .accessibilityLabel("Remove edit: \(draft.words(edit, names: model.editName))")
                    }
                }
                HStack {
                    Button("Review changes…") { model.reviewDraft() }
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(model.editing.held)
                        .accessibilityIdentifier("edit.review")
                    Button("Discard", role: .destructive) { model.discardDraft() }.disabled(model.editing.held)
                }
            } else {
                Text("No edits yet. Edits gather here and apply together after review.").font(.caption).foregroundStyle(.secondary)
            }
            if case .applied(let id, let revision) = model.editing.phase {
                Label("Applied as operation \(id.prefix(13))… at revision \(revision.map(String.init) ?? "?")", systemImage: "checkmark.circle")
                    .font(.caption).accessibilityIdentifier("edit.applied")
            }
            if let sent = model.editing.unresolved {
                Label("The last apply's outcome is unknown; reconcile it before anything else applies.", systemImage: "questionmark.circle")
                    .font(.caption).foregroundStyle(.orange)
                Button("Reconcile") { model.reconcileApply() }.accessibilityIdentifier("edit.reconcile").help("Resend request \(sent.operationId) to learn what became of it")
            }
        }
    }
}

/// The application's review of the draft: what an apply would record, the work affected, and the apply itself.
struct ReviewSheet: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Review plan changes").font(.title3.weight(.semibold))
            switch model.editing.phase {
            case .validating, .drafting:
                ProgressView("Validating with the application…")
            case .reviewed(let preview):
                reviewed(preview)
            case .refused(let refusal):
                Label(refusal.message, systemImage: "xmark.octagon").foregroundStyle(Palette.critical).fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("edit.refusal")
                if refusal.stale {
                    Text("Your edits are kept. Read the current plan and review them again before applying.").font(.callout)
                    Button("Re-read and review again") { model.rebaseDraft() }.accessibilityIdentifier("edit.rebase")
                }
            case .applying:
                ProgressView("Applying…")
            case .applied(let id, let revision):
                Label("Applied as operation \(id) at revision \(revision.map(String.init) ?? "?").", systemImage: "checkmark.circle")
            case .unknown(let sent, let cause):
                Label("The outcome of request \(sent.operationId) is unknown: \(cause)", systemImage: "questionmark.circle").fixedSize(horizontal: false, vertical: true)
                Button("Reconcile") { model.reconcileApply() }.accessibilityIdentifier("edit.sheet-reconcile")
            }
            HStack {
                Spacer()
                Button("Close") { model.closeReview() }.keyboardShortcut(.cancelAction)
            }
        }
        .padding(20)
        .frame(minWidth: 520, idealWidth: 600, minHeight: 320)
        .accessibilityIdentifier("edit.review-sheet")
    }

    @ViewBuilder
    private func reviewed(_ preview: ProposalPreview) -> some View {
        Text("Validated against revision \(preview.revision). Nothing is recorded until you apply.").font(.callout).foregroundStyle(.secondary)
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(preview.changeList.enumerated()), id: \.offset) { _, change in
                    Text(ChangeWords.describe(change, names: model.editName)).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                }
                if !preview.affectedWork.isEmpty { Text("Work to reassess: \(preview.affectedWork.joined(separator: ", "))").font(.caption) }
                if !preview.applicability.isEmpty { Text("Entering or leaving the active graph: \(preview.applicability.joined(separator: ", "))").font(.caption) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(minHeight: 120)
        TextField("Reason (recorded with the change)", text: Binding(get: { model.editing.reason }, set: { model.setEditReason($0) }))
            .textFieldStyle(.roundedBorder)
            .accessibilityIdentifier("edit.reason")
        if let blocked = model.applyBlocker { Label(blocked, systemImage: "lock").font(.caption).accessibilityIdentifier("edit.blocked") }
        HStack {
            Text("As \(model.editing.actor)").font(.caption).foregroundStyle(.secondary)
            Spacer()
            // Return in the reason field must not apply; applying takes an explicit ⌘↩ or a click.
            Button("Apply") { model.applyDraft() }
                .keyboardShortcut(.return, modifiers: .command)
                .help("Apply the reviewed changes (⌘↩)")
                .disabled(model.applyBlocker != nil)
                .accessibilityIdentifier("edit.apply")
        }
    }
}
