# Agent tools and CLI contract

Run `dpm-mcp --actor agent:coder` as a stdio subprocess inside a configured project.
Use `--project DIR` for an exact project, or `--database PATH` (`--db` alias) for a SQLite file.
The same [project discovery](projects.md) rules apply to CLI and MCP; the repository preview rejects
project mutations with `read_only_project` and never creates a project database.
The configured actor is the local principal for every project mutation. A separate verifier process uses
`--actor human:reviewer` or a service actor. This is a trusted local workspace, not remote authentication.
Every CLI mutation likewise requires an explicit `--actor KIND:NAME`; there is no default identity, so
omitting it can never make one caller its own reviewer.

Both adapters use `dpm-app` for queries, revision checks, engine commands and atomic persistence.
The CLI's `--json` output equals the MCP result's `structuredContent.data`. Execution tools add `api_version:7`
and the observed `revision`; every MCP mutation tool requires that `base_revision`, and a call without it
is refused with `invalid_request`. CLI callers
can enforce the same precondition with `--base-revision N`; without it the CLI uses its loaded revision,
which the store still checks atomically. Presentation text is not the API contract.

| CLI | MCP tool | Shared behavior |
| --- | --- | --- |
| export | export_plan | Authoritative snapshot for proposals |
| plan schema | plan_schema | JSON Schema of the plan used by export and proposals; the CLI needs no workspace |
| plan template | plan_template | Minimal valid proposal for a workspace without projects or work |
| plan diff FILE | propose_change | Validate a candidate and inspect entity/field differences |
| plan apply FILE --reason TEXT | apply_change | Human/service applies an observed-revision proposal atomically |
| plan import-mspdi FILE --project-key KEY | import_mspdi | [MSPDI](#microsoft-project-xml-interchange) candidate, preview and per-item report; no state change |
| plan export-mspdi --project-key KEY | export_mspdi | One project's work as MSPDI with a report of omitted data |
| history --after-sequence N --limit N | history | Chronological operation pages with actor, time, reason and command |
| status | project_status | Counts and optional Monte Carlo forecast |
| next --project-key KEY --resource-key KEY | next_work | Full-graph ranking, then [scope](#scoped-next) and limit; outside-scope work stays visible |
| show KEY | get_work | Objective, steps/results, scope, acceptance/checks and derived status |
| explain KEY | explain_work | Readiness, dependencies, resolved requirements/gates/risks/evidence |
| ratify KEY | ratify_contract | Human/service approves a complete Proposed contract |
| reject KEY REASON | reject_work | Independent reviewer (never a holder or evidence author) returns Submitted work for rework |
| claim KEY | claim_work | Reserve only ready tasks; a claim is not a start |
| release KEY --reason TEXT | release_work | Owner gives up an unstarted claim; the task returns to Planned without an owner and the release is recorded |
| handoff KEY --to KIND:NAME --reason TEXT | handoff_work | Human/service moves claimed, started or blocked work to another owner; every recorded fact stays |
| start KEY | start_work | Owner starts claimed work; records the start event SS/SF successors wait for |
| block KEY REASON | report_blocker | Record blocker and preserve owner |
| unblock KEY | unblock_work | Resume without changing owner |
| progress KEY PERCENT --note TEXT | report_progress | Owner reports 0..100 execution; verification remains separate |
| submit KEY --note TEXT | submit_work | Request independent verification of started work once FF/SF gates are released |
| verify KEY --note TEXT | verify_work | Refuse holders and evidence authors; re-check every relation and decision; record the finish event |
| decide KEY OUTCOME | decide_gate | Human/service resolves an open decision (agents are refused); with `options`, OUTCOME is exactly one option key |
| artifact KEY FILE.json | add_artifact | Owner attaches the same Artifact JSON object to its task |
| attach-git-head KEY --resource KEY | attach_git_head | Owner captures HEAD for an explicit task resource; locator binding is the default |
| link-external KEY --provider P --instance HOST --namespace NS --kind K --id ID | link_external | Link work to a provider-scoped external object; context only |
| unlink-external KEY --provider P --instance HOST --namespace NS --kind K --id ID | unlink_external | Remove one link; the work graph is unchanged |
| workspace list | workspace_list | List device-local bindings without changing the plan |
| workspace register --database PATH | workspace_register | Register an existing store; explicit replace redirects a local binding |
| waive-dependency ID --reason TEXT | waive_dependency | Human/service stops enforcing a Soft edge; Hard edges need plan review |
| restore-dependency ID --reason TEXT | restore_dependency | Human/service enforces a waived Soft edge again |
| revalidate-basis KEY --dependency ID --attempt N --reason TEXT | revalidate_basis | Independent human/service re-bases started work after the attempt it relied on was rejected |

Project initialization/import and opening the TUI are local CLI administration. `export_plan` supplies
the full candidate shape for `propose_change` and `apply_change` (argument `plan`). Preserve its
workspace identity and revision; plan apply defaults to the file's revision, never a silently refreshed
one. Diff entries contain collection, stable ID, changed fields and full before/after values; null
means addition/deletion. Preview has no side effects. Applying requires a nonempty reason and a
human/service actor. Agents draft scope; they do not approve their own expansion.

New tasks must be Proposed, without execution/evidence. Existing work keys/kinds, lifecycle, owners,
progress, reviews and artifacts cannot be changed through this route. Started work, its
prerequisites and containing packages, and the projects, requirements and resources it names are
protected, and no new gate may block them; add follow-up work instead. Unstarted contracts,
dependencies, projects, requirements, resources and risks can be maintained after review, but no
actor may apply a change that relaxes a constraint on work it owns or on the work waiting for it
(see dependency policy below). New
decisions are Open questions; decision replacement below may list started work for reassessment.
Git remotes in a shared plan cannot embed `user:password@` credentials. Undo is not part of this route.

A Decided choice is replaced, never edited: the proposal changes only its `status` to `Superseded`
and adds one new Decided decision whose `supersedes` names it, with a nonempty `rationale` and no
`blocks`. The old outcome, rationale and sources stay intact. `explain` returns both records for
every work item linked to either one, following `supersedes` forward, so work linked only to the old
choice still sees its replacement.
Open gates are resolved only by `decide`; superseding one, rewriting a prior decision, dangling or
repeated `supersedes` links, and replacements that add gates are rejected with no state change.
`affected_work` in the preview lists every work item (key, kind, status) whose context contains either
decision, including started work, so reviewers can reassess it; it never changes readiness.
A replacement for a decision with `options` keeps the same option keys, and its outcome may select a
different option: that is the only way to change a choice once made. `applicability_changes` in the
preview lists each work item whose [applicability](#conditional-work-and-branch-joins) the proposal
changes, with `in_flight`, `before` and `after`.

## Release and handoff

`release_work` (`key`, nonempty `reason`, `base_revision`) lets the owner give up a claim it has
not started: the task returns to `Planned` with no owner and is claimable again, and the work's
`releases` list keeps `{actor, at, reason}`. Any actor kind may release its own claim. Anything else is refused with no change: another actor's claim, unowned or
blocked work (unblock first), and started or submitted work (`has started`; use a handoff).

`handoff_work` (`key`, `to` as `KIND:NAME`, nonempty `reason`, `base_revision`) moves claimed,
started or blocked work to another owner. The configured actor must be a human or service; agents
are refused. The command records the observed owner as `from`, and the work's `handoffs` list keeps
`{from, to, actor, at, reason}` for every transfer. Status, events, attempts, basis, progress,
blocker and evidence are unchanged, so the new owner continues where the work stopped. Submitted
work is refused (reject it first), as are unowned work, `to` equal to the owner and a malformed
`to` (`invalid_request`). No actor that has held the work, now or before a handoff or release, may
verify, reject, revalidate its basis or waive its edges, and no author of evidence attached to the
task may verify or reject it. Only a task's current owner attaches evidence to it (`add_artifact`,
`attach_git_head`); unclaimed tasks are refused. A recorded rejection or revalidation stays valid
when its reviewer later takes the work over: independence is judged against the holders at the
review time. Plan changes cannot change owners, handoffs or releases.
See [the decision log](architecture.md#release-and-handoff).

## External tracking references

An external reference records an issue, pull request or other tracker object without making its
state authoritative. Its identity is provider family (`GitHub`, `GitLab`, `Forgejo`, `Gitea`,
`Jira`, `Linear` or `{"Other":"name"}`), `instance` (lowercase `host[:port]` of the hosted or
self-hosted server), `namespace` (owner/repository, group path, tenant or project), `kind`
(`Issue`, `PullRequest` or `{"Other":"name"}`) and `external_id`.

### Provider table

One closed table decides, per provider, which kinds are accepted, the number space of each kind,
the identifier spelling and the namespace rule. Canonical spelling, validation, the collision
check, `link_external`/`unlink_external` lookup, the kind-change rule and reviewed plan changes
all read this table and nothing else. Two identities name one object exactly when their object
keys are equal: (provider family, instance, namespace, number space, normalized id).

| Provider | Namespace | Id | Kinds and number spaces |
|---|---|---|---|
| GitHub | owner/repo, required | number | `issue`, `pull_request`, `discussion`: one space |
| Forgejo, Gitea | owner/repo, required | number | `issue`, `pull_request`: one space |
| GitLab | project or group path, required | number | issue space: `issue`, `incident`, `task`, `test_case`, `ticket`, `objective`, `key_result`, `work_item`; `pull_request` (merge request) space; `epic` space |
| Jira | forbidden (keys name the project) | `PROJECT-N` | every issue type (`bug`, `story`, `sub_task`, `epic`, ...): one key space |
| Linear | workspace, required | `PROJECT-N` | every issue type: one key space |
| Other | optional | opaque | each kind is its own space |

GitHub numbers issues, pull requests and discussions in one repository sequence, and Forgejo
(a fork of Gitea that keeps its paths and numbering) numbers issues and pull requests in one
sequence, so `#5` there has one record and one owner whichever kind is named. Forgejo and Gitea
are one family on one instance; the first recorded family is kept for display. GitLab work items
of project-level types (issue, incident, task, test case, Service Desk ticket, objective, key
result) share the project's issue IID sequence (a Service Desk ticket is tracked "as ticket
`#<issue_iid>`"), merge requests have their own project sequence, and epics have a group-scoped IID
(`GET /groups/:id/epics/:epic_iid`); requirements are numbered separately (`REQ-1`) and are
rejected rather than guessed into a space. Sources: GitLab docs `user/work_items`, `api/epics`,
`user/project/service_desk/using_service_desk` and `user/project/requirements`. Jira and Linear
number every issue type in one key sequence, so the kind is a label there and never part of the key. On GitHub, Forgejo, Gitea and GitLab any kind
outside the table is rejected (`invalid_command`, naming the accepted kinds), so no spelling can
give one number a second identity; Jira and Linear accept any kind name, and have no pull requests.
Every family stays distinct from the others, even on one host, and each instance is separate
(`github.com` and a GitHub Enterprise host never collide).

Kind names are folded by one normalizer: case and separators are ignored and one plural ending is
dropped (`ies` reads as `y`; `ss` is kept). So `pull_requests`, `Pull Requests`, `PRs`, `pulls`,
`merge_request` and `MRs` are `PullRequest`; `issues` is `Issue`; `Work Items` is GitLab's
`work_item`; `epics` is `epic`; `Stories` is `story`. Unlisted kinds keep their folded name.

Identifiers are normalized per provider: forge numbers drop `#`, `!` and `&` prefixes and leading
zeros (`#006` is `6`) and must be positive numbers, so a GraphQL node ID cannot become a second
identity of the same issue; Jira and Linear keys upper-case the project and drop leading zeros of
the number (`proj-06` is `PROJ-6`) and must look like `PROJECT-N` (a letter, then letters, digits
or `_`). Adapters canonicalize equivalent spellings before lookup, and validation rejects any
other form: host and forge namespace case, `https://` prefixes, a trailing `/` or root `.` on the
host, port leading zeros and the default ports `443`/`80` (`:0443` is no port), `www.github.com`
for `github.com`, empty and `.` namespace segments (`./ops//dpm` is `ops/dpm`; `..` is rejected), a
`.git` repository suffix in any case, and Linear workspace case. Ports must be 1-65535. Each rule is
applied until nothing more changes, so canonicalizing a canonical identity leaves it unchanged and
a suggested spelling is always accepted. Commands check the requested identity itself, even when it
resolves to an existing record.

### Linking

`link_external` takes `key`, `identity`, `base_revision` and optional `label`, `url`, `role`
(`Tracks` by default, or `Relates`) and `observed` (`Open`, `Closed` or `Merged`). The CLI uses
lowercase flag values. Each object is recorded once, under a stable reference ID that survives
relabeling and namespace moves; links name work by stable ID, so key changes keep them. One work
item may track an object (`tracking_conflict` otherwise); any number may relate to it. A work item
links an object at most once; the owner may relink with `Tracks` only to restate the kind, and
records its label, URL and observation with it. The kind-change rule is the table's: a kind may
change to another kind of the same number space (an issue refined to a pull request, a GitLab issue
restated as an incident, a Jira bug relabelled as a story), except that a pull request is the most
specific kind of its number and is never restated as anything else. `Relates` links never change
the recorded kind. `Merged` is final: a merged pull request can neither reopen nor close, so after
a `Merged` observation any link reporting `Open` or `Closed` is rejected (`invalid_command`); link
without `observed` to add context. `unlink_external` removes one link, and removing the last one
removes the record. Unknown work, identities or links are `not_found`; stale revisions are
`revision_conflict`. Every failure leaves the snapshot and revision unchanged.

Links are never evidence, dependency satisfaction, gates or verification. `observed` is an
attributed report: a closed issue does not complete work, and a merged pull request does not
submit or verify it. Attach a pull request as an Artifact through `add_artifact` when it is
evidence; a separate verifier still decides. `explain.context.external_references` lists references
linked to the work or its containing packages, while `next`, `status` and schedules ignore them.
No connector, token store or assignee write is involved.

### Credentials, labels and URLs

Credentials are rejected by one detector, applied to every label word and to the whole URL. Each
word is percent-decoded until stable (deeper nesting is rejected) and `\` is read as `/`. Then:

- Userinfo: after an optional `scheme:` and any number of slashes (`https://`, `https:/`,
  `https:\\`, `//`, or none as in `user:secret@host`), the authority runs to the next `/`, `?` or
  `#`, and any `@` in it is userinfo. Anywhere else in the word (a query value, a URL nested in a
  query, a word without a scheme) an `@` after a name and before a host (a dotted name,
  `localhost`, `[IPv6]` or `name:port`) is userinfo unless it is a bare address: nothing but the
  end of the word or another parameter right after the host, no port, no `:` in the name
  (`tok@github.com/o/r`, `?next=https://tok@evil.example/x` and `user:pw@host` are rejected;
  `?jql=reporter=alice@corp.example` is accepted). Userinfo is rejected unless it is exactly `git`,
  the public SSH user of every forge (`git@github.com:o/r.git`). An email address
  (`user@example.com`, also after `mailto:`) and a fediverse handle (`@alice@mastodon.social`) are
  allowed; this includes a token-shaped local part such as `ghp_x@github.com`, which carries no host
  path to authenticate.
- Secret parameters: the word is split at `?`, `&`, `;`, `/` and `#`, and a `name=value` piece
  with a nonempty value is rejected when its name is a secret name. The name is split into words at
  `_`, `-`, `.`, `:` and lower-to-upper case changes; trailing digits of each word are dropped
  (`token2`, `password1`), and trailing qualifier words (`value`, `val`, `str`, `string`, `id`,
  `data`, `raw`, `hex`, `b64`, `base64`, `text`) are dropped while a word remains before them
  (`tokenValue`, `session_id`). The last remaining word is a secret when it is `key`, `apikey`,
  `sig`, `pwd`, `pass`, `jwt`, `auth`, `sid` or `bearer`, or ends with `token`, `secret`,
  `password`, `passwd`, `signature`, `session`, `sessionid`, `sessid` (`PHPSESSID`, `JSESSIONID`),
  `credential(s)` or `authorization`, or is the OAuth `code` alone or after `auth`, `oauth`,
  `access`, `authorization`, `device` or `verification` (`auth_code`). So `access_token`,
  `accessToken`, `API.KEY`, `client_secret`, `private_token`, `X-Amz-Signature`, `--token` and
  `%74oken` are secrets, while `max_tokens`, `secret_santa`, `language_code`, `user_id` and anchors
  such as `#token-refresh` or `#api-key-setup` are not. Any `*_key` name (`sort_key`) counts as a
  secret.
- Intentional misses: a secret value under an ordinary name (`?q=ghp_abc`), a bare token in a
  label (`ghp_abc`), a secret in a path segment without `=` (`/token/abc`), one-letter or
  unrelated names (`?k=`, `?x=`), OAuth `state`, and a `name@host` pair with no path, port or `:`
  inside a query value (`?next=tok@evil.example`), which reads as an address. The detector
  recognizes credential syntax, not credential content; it is a guard against pasting live links,
  not a secret scanner.
- Intentional false positive: an address after a `:` inside a query value
  (`?q=author:alice@corp.example`) reads as `user:pw@host` and is rejected; that is the price of
  rejecting `?x=user:pw@evil.example`.
- Intentional false positive: an `@` anywhere after a `//` in the same word reads as userinfo in
  an authority and is rejected, so `see //TODO@alice`, `a//b@c` or `[a](https://example.com)@bob`
  fail; that is the price of rejecting `x=https://tok@host` and `see:https://tok@host`. Separate the
  words with a space.

The URL must also pass structural rules, so it is never looser than a label: `http://` or
`https://` followed directly by an authority equal to the identity's instance after the same
canonicalization (no userinfo, no encoding; `www.github.com` is `github.com`); a decoded path of
letters, digits and `-._~/:+,` only (no `;` parameters, `=` or `@`); any query the detector does not
flag, with values that may contain `:` and parameters that may be empty; and an optional fragment
that is a plain anchor (letters, digits, `-._~`). Real provider links such as
`?notification_referrer_id=…&utm_source=email`, `checks?check_run_id=1`, GitLab
`diffs?commit_id=…`, Forgejo `files?style=split&whitespace=`, Jira `boards/1?selectedIssue=PROJ-6`
and the Jira comment permalink
`?focusedCommentId=10&page=com.atlassian.jira.plugin.system.issuetabpanels:comment-tabpanel#comment-10`
are accepted.

### Reviewed plan changes

References are part of the exported plan, so reviewed plan changes can add, relabel, move or
remove them under the same validation. `plan diff` reports them under `external_references`.
A proposed record is compared with every current record it continues: the one with its reference
ID (a relabel or namespace move) and the one with its object key (a removal and re-addition under a
new ID). Either way it is an edit of that object, so it follows the link rules: the kind may only
change as linking would change it (a pull request is never downgraded), and, like evidence and
reviews, observations are attributed records that a plan change cannot add, rewrite or drop;
record them with `link_external`. Under the same reference ID, only a namespace move (repository
transfer) or an instance change (server migration) keeps the object; another number, number space
or provider names a different object, so a record carrying an observation cannot be rewritten into
it — unlink it and link the other object. A removal in one reviewed change followed by a re-addition in a
later one is indistinguishable from unlinking the last link and linking again, which starts a new
record without history.

### History

`history` returns entries in append order with a `next_after_sequence` cursor (default limit 100,
capped at 1000). Sequence is local to the store, distinct from wrapping revision IDs. Snapshot export
is not an operation backup; use the CLI's `backup`, `restore` and `verify-store`
([recovery](projects.md#backup-restore-and-verification)).

## Microsoft Project XML interchange

The supported external format is the documented Microsoft Project XML schema (MSPDI), which
Microsoft Project, ProjectLibre, OmniPlan Pro and MPXJ read and write. OmniPlan requires its Pro
license for XML import/export. Binary `.mpp` and Primavera
files are not supported; convert them to MSPDI with another tool first. Tests read documents
written by MPXJ and by OmniPlan 4.10.3, check that MPXJ reads DPM's export back unchanged, and
record OmniPlan's reading of DPM's export (fixtures README); acceptance by Microsoft Project itself
is not verified. `dpm-interchange` parses
and writes the subset without network access and never touches the store.

`plan import-mspdi FILE --project-key KEY [--key-prefix P] [--match-existing-by title-path]
[--keep-existing-priority] [--candidate OUT.json]` and `import_mspdi` (`xml`, `project_key`,
optional `key_prefix`, `match_existing_by` and `keep_existing_priority`) return
`{report, preview, candidate}`.
`candidate` is a full plan at the observed revision, `preview` is its `propose_change` result, and
`--candidate` also writes it to a file. Importing is a query: nothing is persisted until a human or
service applies the candidate with `plan apply` / `apply_change`, which re-validates it against
the current revision. A malformed document, an unknown project or a stale candidate leaves the
revision and operation history unchanged.

| MSPDI | DPM candidate |
| --- | --- |
| Task `GUID` | Work identity. Without one, an identity derived from the project `GUID` and task `UID`; without either, one derived from the target project, the explicit key prefix and the task `UID` (see [identity without GUIDs](#identity-without-guids)) |
| `OutlineLevel` order | `parent`; a summary task (or any task with children) becomes a WorkPackage |
| `Milestone=1` | Milestone; a nonzero source duration is dropped and reported. Without the element, existing work keeps its kind (reported as `kept`) and new work defaults to Task (reported as approximated) |
| other tasks | Task, `Proposed`, empty acceptance: never executable until ratified. A zero-duration task without the flag stays an unestimated Task, so exported unestimated tasks keep their kind |
| `Name`, `Notes` | `title`, `objective`; an empty name or absent notes keep the local value. Trailing line breaks in `Notes` are not content (OmniPlan ends every note with one) and are dropped |
| `Priority` 0..1000 | P0 ≥800, P1 ≥600, P2 ≥400, P3 ≥200, else P4; export writes 900/700/500/300/100. Absent: existing work keeps its priority; new work gets the MSPDI default 500 (P2), reported as approximated. OmniPlan rescales on export: it writes ⌊level·1000 ÷ highest level in the document⌋ with level = priority ÷ 100 (the `UID` 0 summary does not count), so the highest priority always comes back as 1000. With a P0 in the plan, DPM's 900/700/500/300/100 come back as 1000/777/555/333/111 in the same bands; a plan without P0 comes back one band higher (700/500/300 as 1000/714/428); groups may come back as 0 (P4). `--keep-existing-priority` / `keep_existing_priority: true` keeps the priority of existing work, lists `priority` as `kept` and reports the differing source value as approximated; new work still maps its source value |
| `Duration` `PTnHnMnS` | Single-point estimate O=M=P in hours; zero means unestimated |
| `PredecessorLink` `Type` 0/1/2/3 | FF/FS/SF/SS dependency |
| `LinkLag` | `lag_hours = LinkLag / 600` (tenths of a minute) |

Interchange preserves plan structure, not calendar dates. Source start and finish dates are ignored
because DPM derives dates from the graph; working-time durations and lags become continuous elapsed
hours; calendars (including resource calendars), resources, assignments and date constraints are
reported as not imported. `report.source.scope` states this boundary in every import report.

DPM schedules elapsed hours. Elapsed duration/lag formats (`em`, `eh`, `ed`, `ew`, `emo`) convert
exactly. Working-time formats (`m`, `h`, `d`, `w`, `mo`) keep their hour value but lose the
calendar: 1 working day (`LinkLag` 4800 on an 8-hour calendar) becomes 8 elapsed hours, not a
calendar day. The report marks those values as approximations. `LinkLag` always counts tenths of a
minute; `LagFormat` only selects the display unit and elapsed versus working time, so a lag without
`LagFormat` (OmniPlan writes every lag that way) is Microsoft Project's default, working time, and
is approximated like any other working-time lag. Percentage lags and unknown formats are rejected
with the link rather than guessed. Durations
with day, week or month designators are rejected. Cross-project links are rejected.

Work packages cannot be dependency endpoints. A summary finishes with its last child and starts
with its first, so a summary predecessor of an FS or FF link and a summary successor of an FS or
SS link expand exactly into one edge per task/milestone descendant. The other combinations, links
between a summary and its own descendant, and summaries without imported descendants are rejected.
Relations that expand onto the same pair and kind merge, keeping the larger lag; every link merged
that way names the shared dependency, and a link whose own lag is not the one carried is
`Approximated` with the merged lag in its notes. A link that changes the lag of an existing local
edge is never `Preserved`: it is `Changed` (or `Approximated`, when its own lag is approximated) and
its `changes` list the dependency, relation, field and `before`/`after` values.
`report.removed_dependencies` lists local edges between imported work that the candidate removes
because the document omits them.

`report.items` has one entry per source task with `outcome` (`Created`, `Updated`, `Unchanged`,
`Skipped`), the mapped `work`, and `preserved`, `approximated` and `rejected` findings. Fields the
source omits never count as preserved: `kept` names them, and existing work keeps the local value.
`changes` lists every local field an `Updated` item changes (`title`, `objective`, `kind`, `parent`,
`priority`, `estimate`) with `before` and `after` values, and is empty otherwise;
`report.links` has one entry per `PredecessorLink` with `outcome` (`Preserved`, `Changed`,
`Approximated`, `Rejected`), notes, `changes` to existing local dependencies and the resulting
dependency IDs. Rejected data includes constraints,
deadlines, calendars, resources and assignments, baselines, custom fields and outline codes,
manual scheduling, recurrence, cost, timephased data, and percent complete or actuals. Source
progress never submits, verifies or completes local work. Inactive, blank, external and subproject
tasks are skipped with a reason; `report.rejected` covers document-level data and
`report.retained` lists local work in the project that the document does not name.

### Identity without GUIDs

Some tools, OmniPlan among them, write neither a project `GUID` nor task `GUID`s. `UID`s are unique
only within one file, so for such a document the key prefix names the source: identity is derived
from the target project, the key prefix and the `UID`, and the new key is `PREFIX-UID`. The prefix
is then required rather than defaulted (`invalid_request` otherwise), because a default would make
every GUID-less file imported into a project the same source and silently merge unrelated tasks
that share a `UID`. Re-importing the same file with the same prefix updates the same work and never
duplicates it; another prefix plans separate work. Reusing a prefix for a different GUID-less file
asserts that it is the same source, and every item's `identity` finding says so. A prefix already
used by an import that carried GUIDs cannot be reused: the new `PREFIX-UID` key collides and the
import is refused. Renumbered `UID`s import as new work. The `UID` 0 / `OutlineLevel` 0 task
summarizes the whole document; the target project stands for it and it is reported as skipped.

A round trip through such a tool (DPM export, edit in OmniPlan, import) loses the GUIDs DPM wrote,
so by default the file comes back as a new source and plans new work beside the original. Mapping it
back onto the original work is an explicit request, never a heuristic applied silently:
`--match-existing-by title-path` / `match_existing_by: "title-path"` matches each task without a
`GUID` to the one work item in the target project whose titles from the project root down equal the
task's outline path of names. Every match is an `identity` approximation naming the matched key and
path; a task without a match keeps its derived identity and says that no work has its path. A path
shared by several local items, or by several GUID-less tasks that could match, or a task whose
derived identity already names other work than its path matches, refuses the whole import
(`invalid_command`) listing every ambiguity, because a partial or guessed merge would silently move
history, evidence and ownership onto the wrong work. Titles are the one thing every tool round-trips;
outline numbers and `UID`s are renumbered freely, and dates and durations are exactly what an edit
changes. Renaming a task in the other tool therefore turns it into new work, and the report says so.
Priorities from such a tool may be relative (see `Priority` above); against work with execution
history a raised band is a protected change that refuses the whole import, so an OmniPlan round
trip of a plan in progress needs `--keep-existing-priority` as well.

Re-importing updates work with the same identity and never duplicates it. Keys, lifecycle,
ownership, progress, evidence and acceptance stay local. The document owns only the dependencies
between work it imports; other edges are kept. An unchanged source duration or lag (compared in the
exported encoding) keeps the richer local three-point estimate, exact lag, policy, rationale and
waiver. Local work the document omits is retained, not deleted. Changes that touch protected
(started) work are refused by `propose_change` as with any reviewed change; when the refused work is
an imported task, the error also names the source `UID` and `GUID` and the attempted field and
dependency changes. When the refusal names a dependency, such as a waived soft edge that the
document would re-lag or drop, the error names the source link instead: the predecessor and
successor `UID`, `GUID` and work key, and the attempted change or removal; the engine refusal stays
its cause. A document that would change the kind of existing work (a task gaining children,
a milestone flag flipped) fails the import with the same source context.

`plan export-mspdi --project-key KEY [--output FILE]` and `export_mspdi` (`project_key`) return
`{xml, report}`; without `--json` the CLI prints the document itself. Output is deterministic:
siblings follow natural key order (digit runs compare by value, so `OP-2` precedes `OP-10`; the
same order as the terminal outline), `UID`s number that order (they are local to the file), and `GUID`s are
the stable project and work identities. Tasks carry the PERT expectation in elapsed hours, and
links carry elapsed-hour lags. The report lists per-item omissions (acceptance, instructions,
lifecycle, owner, requirements, evidence, resources) and project-level data outside the subset.
MSPDI has no conditional work: every task is written unconditionally, never dropped, and the item
report names its `condition`, an active-branch `join`, and any current non-applicable state
(`applicability`); a link from not-selected work carries a note that it is written as enforced.
Exporting a project and importing the document into the same workspace yields no changes.

## Dependency policies, waivers and links

Every dependency in `export_plan` and `explain_work.context.dependencies` carries a stable `id`,
`policy` (`Hard` or `Soft`), optional `rationale` and, while set aside, a `waiver` with actor, time
and reason. An ordered task pair holds at most one edge per relation kind, so SS and FF coexist;
`propose_change` reports each edge as its own `dependencies` entry keyed by `id`. Commands name
edges by `id` (argument `dependency`); malformed or absent IDs return `not_found`. Proposals may
omit `id` on new edges, which then receive a deterministic identity from predecessor, successor and
kind. Duplicate identities or relations, missing endpoints, cycles and stale revisions reject the
whole proposal.

Policy, kind, lag and rationale change only through reviewed `apply_change`, and never by an owner of
either endpoint in the relaxing direction: removing the edge (also by deleting its other endpoint),
Hard to Soft, a lower lag, a relation that no longer implies the old one (FS implies SS and FF, each
implies SF) or a provisional start basis is refused as `invalid_command` with
`details = {work, relaxed: {type: "dependency", dependency, predecessor, successor, relaxation}}`;
a change that would release a gate unmet at apply time on the actor's own work, or on a direct
successor because of it, reports `relaxed: {type: "gate", work, transition, gate}`. An equally strict
or stricter edge under any identity is accepted, and another human or service may apply the same
proposal. Proposals cannot add,
alter or remove waivers, nor edit or remove a waived edge, which must be restored first. `waive_dependency` and `restore_dependency` take `dependency`, a nonempty
`reason` and `base_revision`. Only human/service actors that neither hold nor have held (before a handoff or release) either endpoint task may use them
(`verify_work` differs: it refuses only holders of the verified task, so a successor's owner may verify
the result it consumes; `revalidate_basis` refuses holders of either task), only Soft edges can be waived,
and restoring requires a current waiver. The waiver's actor, time and reason stay on the edge until
restoration; `history` records both operations with actor, time and reason. A waived edge is absent
from `gates.unmet`, readiness, verification prerequisites, milestone completion and the remaining
forecast. An unwaived Soft edge gates exactly like a Hard one, and `gates.unmet` reports its
`dependency` and `policy`.

`links` in the plan are typed non-gating relationships (`RelatesTo`, `Duplicates`, `DerivedFrom`,
`Supersedes`) with `source`, `target` and optional `note`, edited through reviewed plan changes.
Endpoints must exist and differ; one pair holds at most one link of each kind in either direction.
`explain_work.context.links` lists those touching the work. Links never change readiness,
scheduling, ranking or progress.

## Conditional work and branch joins

A decision may list structured `options` (`[{key, label}]`, at least two, unique trimmed keys).
Once decided, its `outcome` is exactly one option key; `decide` refuses any other outcome with no
state change. A work item's optional `condition: {decision, option}` makes it, and everything a
work package contains, apply only when that option is selected; conditions on nested packages all
apply. A task or milestone may set `join: {"mode": "active_branches", "allow_empty": false}`; the
default (`all_predecessors`, omitted) keeps ordinary dependency semantics. All three fields are
additive and edited through `propose_change` / `apply_change`.

Applicability is derived at query time and never stored. `explain_work.applicability` and
`project_status.not_applicable` report it with `state`:

| State | Meaning | Transitions | Forecast | Progress |
|---|---|---|---|---|
| `applicable` | Selected, and every prerequisite can proceed | gated as usual | included | counted |
| `undecided` | A condition awaits an open decision | refused | excluded; see scenarios | not counted; container/workspace incomplete |
| `not_selected` | A decision selected another option | refused | excluded | not counted; never completion |
| `awaiting_choice` | A prerequisite (or, for a work package, a child) is undecided | refused | excluded; see scenarios | counted |
| `stranded` | An ordinary edge from not-selected or stranded work never releases | refused | excluded | counted, outstanding |
| `empty_join` | Every branch into an active-branch join was not selected and `allow_empty` is false | refused | excluded | never reached |
| `all_children_excluded` | Every child of a work package was excluded; `decisions` names the excluding decisions | — | excluded | not counted; never completion; not required for workspace completion |
| `children_stranded` | No child of a work package is applicable and `child` is stranded or an empty join | — | excluded | never complete |

A refused transition reports `{type: "applicability", applicability}` in `unmet`. A dependency from
not-selected work reports `release.state = "not_selected"` into an ordinary successor and
`"skipped_branch"` (released at the choice time) into an active-branch join. A join is reached when
at least one branch is verified (or `allow_empty` is set and every branch was skipped), every active
branch is released, and its gates are resolved; its time includes the effective time of the choices
that selected it or skipped its branches: the earliest `resolved_at` in the unbroken run of equal
outcomes along the `supersedes` chain, so a replacement reaffirming the standing outcome moves no
completion and re-closes no released gate, while a changed outcome counts from its own resolution. A work package completes when every child that a choice
did not exclude is complete, with at least one; its applicability follows the same children: with
no applicable child it is `all_children_excluded` (every child excluded, handled like
`not_selected`, including as a skipped branch of its parent package), `awaiting_choice` (a child
still waits for a choice; `predecessor` names it) or `children_stranded`. These package states are
additive values of `state`; `api_version` is unchanged. `progress.scope` is `not_selected` (also for
`all_children_excluded`) or `undecided` for work outside the counted scope.

`next`, scoped `next`, `status`, `explain`, progress, milestone completion, the remaining CPM and
Monte Carlo and the TUI read one evaluation. While open decisions condition work,
`project_status.open_choices` lists them with `scenario_count` and one `scenarios` entry per option
combination (up to 16): `choices`, `expected_finish_hours`, optional percentiles and the work that
would be `stranded`. The headline `expected_finish_hours` then covers committed work only and the
headline percentiles are null: branches without a probability model are never blended. Float and
criticality in `next` and `explain` are likewise computed over committed work. Lifecycle counts
(`in_flight`, `blocked`, `awaiting_verification`) cover the same scope as `complete` and progress:
work whose own conditions are selected. Claimed, started, blocked or submitted work whose lifecycle
a reviewed choice change kept outside that scope is counted only in `excluded_in_flight`; the TUI Now
page scopes its blocked and needs-review lists the same way and lists that work apart. A replacement decision carries
no `blocks`, as for any replacement.

`decide` refuses a choice that would exclude claimed or started work. Changing a made choice is a
reviewed decision replacement; it keeps in-flight work's lifecycle, owner and evidence, lists it in
`applicability_changes`, and that work then refuses further transitions until the plan changes.

## Prepared self-host example

The default `demo` is the [dpm roadmap](../examples/self-host/README.md), with open execution
and phase gates. Its `next_work` result intentionally has no candidates. Query its contracts freely, but do
not call mutation tools on it until the user separately authorizes beginning the self-host work.
The workflow below applies to an authorized execution workspace; tool availability is not approval.

## Agent workflow

1. Call project_status and next_work. Supply actual capabilities; an empty capability set is the
   unfiltered operator view, not a claim that the actor has every skill. Default limit is 5.
2. Call explain_work. Read `work.objective`, `work.instructions`, `work.acceptance`, predecessor
   evidence and unresolved gates. Steps describe the procedure, not permission to execute it.
3. Call claim_work using the observed revision. Refresh after a revision_conflict. A claim only
   reserves the task.
4. Call start_work when execution begins. Its operation time is the start event that SS/SF
   successors wait for; report_progress and submit_work are refused until the task has started.
5. Perform the work and acceptance checks. Use report_progress for intermediate execution reports;
   use add_artifact or attach_git_head to record evidence while you own the task.
6. Call submit_work with an evidence summary once `explain_work.transitions.submit.ready` holds.
   Another configured actor verifies the result.
7. Call report_blocker when blocked; do not silently ignore dependencies or change lifecycle fields.

`probabilistic:false` matches CLI `status --no-simulation` or `next --deterministic-only`.
Capabilities and limit map to repeated `--capability` and `--limit`; `project_keys` and `resource_keys`
map to repeated `--project-key` and `--resource-key`. Work identifiers in tool arguments
are human keys; returned records also include stable UUIDs. Domain errors use stable `code` plus
human-readable `message` and the `api_version`; the CLI `--json` output is `{"error": {...}}` and MCP
uses `isError:true` with the same object as structuredContent. When `--json` appears anywhere before
`--`, CLI argument errors (a missing `--actor`, an unknown flag, an invalid value) use this envelope
with code `invalid_request` and exit status 2; without `--json` they remain clap's text on stderr.
A command refused by its readiness gates (dependencies, decisions, applicability, provisional basis
or lifecycle eligibility checked by the gate evaluator) also carries `details: {transition, unmet}`,
the same structured conditions `explain_work` reports for that transition. Refusals decided before
the gates run, such as a lifecycle transition from `Blocked`, a missing start or another actor's
ownership, return only `code` and `message`; read `explain_work` `transitions` for the full list.

## Execution events and elapsed lag

Lifecycle commands record their own operation time: `start_work` sets `work.events.started_at`,
`submit_work` sets `submitted_at` (cleared by `reject_work`), `verify_work` sets `verified_at`, and
`decide_gate` sets the decision's `resolved_at`. No tool accepts an event time; plan changes cannot
add or rewrite these fields (omit `resolved_at` from a replacement decision: `apply_change` records
the operation's own time as its `resolved_at`), and a command whose
time precedes the event it follows is refused unchanged. `report_blocker` on work that started
before start times were recorded sets `work.events.start_unrecorded`: the work still counts as
started for its SS/SF successors and `unblock_work` returns it to `InProgress`, while positive lag
from its unknown start time stays closed. Work already blocked in a snapshot saved before
`start_unrecorded` existed has no start fact, so it counts as unstarted for its SS/SF successors, `unblock_work` returns it to
`Claimed` once, and its owner calls `start_work` again, which records a new start time. Every query evaluates gates at one clock reading taken by
the adapter for that response.

| Relation | Gates the successor's | Waits for the predecessor's |
| --- | --- | --- |
| FS | claim and start | verification (`verified_at`) |
| SS | claim and start | start (`started_at`) |
| FF | submission | verification (`verified_at`) |
| SF | submission | start (`started_at`) |

Verification re-checks all four relation kinds and decisions. Only verification is a predecessor's
finish. Positive lag must elapse in calendar time after the event (`release.state: elapsing` with
`event_at` and `opens_at`); negative lag affects the schedule only and never releases work before the
event, which `why_now` states. Tasks verified or started before event times were recorded, and
decisions resolved before then, count as having occurred at an unrecorded time: zero or negative lag
releases, positive lag reports `release.state: unrecorded_event_time` with an actionable reason.
The remaining schedule in `project_status`, `explain_work.schedule`, `next_work` and the Gantt
never releases a constraint before the gate does: SS/SF lag from a started predecessor, like any
lag from verified work, counts from the recorded event and is dropped once released, and a lag from
an unrecorded event time is kept whole. FS/FF edges keep their full lag until the predecessor is
verified, so with a provisional start basis the forecast errs late: the successor may already have
started on the pending submission while the forecast still waits for verification. A started
predecessor that still has outstanding constraints of its own is projected after them, which also
only delays the forecast.

`explain_work.transitions` reports `claim`, `start`, `submit` and `verify` with the same shape as
`gates` (the claim report). A milestone's `progress.completed_at` is the latest release among its
incoming edges and gating decisions, so a decision resolved after every prerequisite sets it;
`{"recorded": TIME}` or `"unrecorded"`.

## Provisional submission bases

Each `submit_work` appends a submission attempt to `work.attempts`: a 1-based `number`, its
`submitted_at` and an `outcome` whose `state` is `pending`, `rejected` (with reviewer, time and
reason) or `verified` (with verifier and time). Reviews close attempts but never remove or renumber
them, so rejection history stays in the snapshot. Submissions recorded before attempts existed have
no attempt record and remain valid.

A finish-to-start edge between two tasks may carry `start_basis: "Provisional"`, set or cleared
only through reviewed `apply_change` like `policy` (and, like every edge into started work, frozen
once its successor is claimed). For the successor's `claim` and `start` gates it releases on the
submission of the predecessor's current (pending or verified) attempt plus positive lag, so
verifying that attempt never delays an elapsing start; `gates.provisional[]` names each edge
released on a still-pending attempt with the `attempt` relied on, and `why_now` and `next_work`
reasons say so. A legacy submission without
an attempt record, or a predecessor a choice did not select, does not release it. `submit`, `verify`, milestone completion, progress and the
remaining forecast still wait for the predecessor's verified finish: in `transitions.verify` the
edge appears with `start_basis: "Provisional"` and no `accepts_submission`.

`start_work` records the attempt each provisional edge was released on as an immutable entry in
`work.basis` (`dependency`, `predecessor`, `attempt`, `recorded_at`, `source.kind: "start"`). Claims
record nothing because they only reserve work. Whether a basis is invalidated is derived, never
stored: when the relied-on attempt is rejected, `transitions.submit` and `transitions.verify` report
`type: "basis_invalidated"` with the edge, `attempt`, the rejection in `state` and the
predecessor's `current_attempt`, and `project_status.basis_invalidated` counts such work. A later
submission or verification of the predecessor never clears it. `explain_work.basis.relies_on` lists
the work's own effective bases and `basis.relied_on_by` the downstream work relying on its attempts,
including the rejected ones, read from the snapshot rather than from history. A waived edge's basis
is reported with `enforced: false` and gates nothing.

`revalidate_basis` (`key`, `dependency`, `attempt`, nonempty `reason`, `base_revision`) appends a
new basis with `source.kind: "revalidation"`, actor and reason. It is refused with no change unless
the actor is a human or service that neither holds nor has held either task (accepting work built on a rejected result is
an accountable judgement, like a waiver, and neither owner may certify its own work), the effective
basis on that edge is invalidated, and `attempt` is the predecessor's current pending or verified
attempt. Revalidating onto a pending attempt relies on it again: if it is also rejected, the work is
flagged again. Nothing rewrites lifecycles in cascade; the successor keeps its state throughout.

## Scoped next

`next`/`next_work` returns one object with `result_version: 1`. Evaluation order is fixed: gates,
readiness and scores use the full workspace graph; capability eligibility then selects the eligible
set; scope partitions it; `limit` truncates only the in-scope list. Scope is query-only: it never
changes claims, graph membership, readiness or scores, and it grants no filesystem authorization.

| Field | Meaning |
| --- | --- |
| result_version | Shape version of this object, independent of `api_version` |
| scope.projects[] / scope.resources[] | Resolved `{id, key}` entries actually applied; both empty means unscoped |
| capabilities | Capabilities used for eligibility; not a scope axis |
| limit | Maximum in-scope candidates returned |
| eligible_count | Ready, capability-eligible work in the whole workspace |
| in_scope_count | Eligible work inside the scope, before `limit` |
| candidates[] | In-scope work in global rank order; each is a ranked candidate plus `global_rank` (1-based among all eligible work) |
| outside_scope.count | Eligible work excluded by scope |
| outside_scope.higher_ranked_count | Outside work ranked above the best in-scope eligible work (before `limit`), or all outside work when none is in scope |
| outside_scope.keys[] | Keys of eligible outside work in global rank order; the first `higher_ranked_count` rank higher |

Membership, applied to eligible work; each empty axis does not filter and non-empty axes intersect:

- **Project** (`--project-key`, `project_keys`): the work's project is a listed project or a descendant.
  `--project DIR` still selects a project directory; the key filter is a separate option.
- **Resource** (`--resource-key`, `resource_keys`): the work names at least one listed resource, and every
  resource it writes is listed. Reads of unlisted resources are allowed. Work naming no resources
  (for example non-code work) never matches a resource scope; if eligible it is in `outside_scope`.
- **Capability** (`--capability`, `capabilities`): eligibility, not scope. Work the caller cannot
  perform is neither a candidate nor counted in `outside_scope`.

Unknown project or resource keys fail with `not_found` instead of matching nothing. A dependency
outside the scope is evaluated like any other: in-scope work waiting on it stays unready, and the
blocking work appears in `outside_scope` when it is itself eligible.

## Task instructions

`show/get_work` returns a WorkItem with derived `progress`. `explain/explain_work` returns that
contract under `work`, alongside readiness/ranking and resolved `context`. Both adapters serialize
the same types and never synthesize or truncate task instructions.

| Field under WorkItem | Meaning |
| --- | --- |
| objective | Purpose and observable goal |
| instructions.steps[] | Ordered actions, each with `action` and `expected_result` |
| instructions.in_scope[] | Permitted changes/deliverables |
| instructions.out_of_scope[] | Explicit exclusions |
| instructions.verification[] | Checks and evidence to collect |
| acceptance[].text | Conditions a separate reviewer must assess |
| capabilities / requirement_ids / artifact_ids | Skill requirements and stable context references |

Instructions are optional for compatibility with plans that omit them. When present, they are valid
only on Tasks and must include nonempty steps/actions/results, both scope lists and verification
checks. Every self-host task supplies all fields. Instructions are author-written data, not a script
runner, execution authorization or automatically verified evidence. If instructions are missing or
contradict gates/acceptance, report the gap instead of guessing permission or completion.

`python3 scripts/smoke_self_host.py` imports the prepared plan into temporary storage, compares every
task contract through real CLI/MCP processes and confirms no operations or revision changes.

`context.decisions` includes both gates and related design choices. `blocks` controls readiness;
`related_work` only supplies context, including through parent work packages. Optional `rationale`
and `artifact_ids` provide the reason and sources, resolved under `context.artifacts`. These sources
are planning context, not task completion evidence. A decided choice does not authorize execution
of the task or resolve any other gate. Missing optional fields retain their empty/default meaning.

## Progress and schedule indications

`report_progress` takes `key`, integer `percent` (0..100), required `base_revision`, and optional
`note`. The configured actor must own the started (in-progress, or blocked after starting) task.
Reports on planned, claimed-but-unstarted, submitted, verified, container or milestone work are rejected. Reports may correct the percentage
downward; blocked reports retain the blocker. Each successful report is a semantic operation.

`status`, `show` and `explain` include `progress: {percent_complete, verified, completed_at}`. Task percentages
reflect `reported_progress_percent` until submission, when execution displays 100%. Only independent
verification satisfies successor execution prerequisites. Work-package/workspace percentages equally
weight descendant leaf tasks; milestone percentages are 0 or 100 according to their derived condition.
A package can have 100% execution with `verified:false` while reviews or gates remain unresolved.
An empty package/workspace reports 0%. A taskless package uses its binary aggregate condition.

Old snapshots that omit `reported_progress_percent` load as zero; lifecycle status still determines
submitted/verified display. Imported reports above 100 or nonzero reports on unowned/aggregate work
are rejected. The report is an additive serialized field; no stored schedule dates are introduced.

`explain.context.dependencies` retains full FS/SS/FF/SF types and signed hour lag; milestone kinds,
zero-duration schedules and derived states are exposed by the same queries. Execution gates follow
[execution events and elapsed lag](#execution-events-and-elapsed-lag); progress reports never
release a gate or shorten the remaining-duration forecast.

## Authoring a plan

`plan schema` / `plan_schema` returns the JSON Schema (draft 2020-12) of the portable plan: the shape
of `export`, and of the `plan` that `plan diff` / `propose_change` and `plan apply` / `apply_change`
accept. Unknown or misnamed fields are rejected, as the model rejects them. Fields the schema marks
"managed" are written by execution commands; keep them as exported. In a workspace without
projects or work, `plan template` / `plan_template` returns a minimal valid proposal with the
workspace identity and revision: a project, a package, two Proposed tasks with complete contracts,
a milestone, a requirement, a decision gate and a risk. Its identifiers derive from the workspace
identity, so both adapters return the same document. Once the workspace has projects or work the
template is refused with `invalid_request`: applying it would delete them, so edit `export` instead.

```sh
dpm init "Project name"
dpm plan template --json > plan.json    # replace the TEMPLATE-* text and add work
dpm plan diff plan.json --json
dpm plan apply plan.json --reason "Initial plan" --actor human:NAME
dpm ratify TEMPLATE-DESIGN --actor human:NAME
```

## Protocol

The runnable adapter implements MCP `2025-11-25` initialize/initialized, ping, tools/list and tools/call.
Messages are newline-delimited UTF-8 JSON-RPC, limited to 8 MiB. Stdout is protocol-only. Initialize
with protocolVersion, capabilities and clientInfo, then send notifications/initialized. Responses
include text content and structuredContent. Domain failures are tool errors; malformed protocol
requests use JSON-RPC errors. EOF cleanly stops the process. The adapter advertises only tools.

Specification:
- https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
- https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle
- https://modelcontextprotocol.io/specification/2025-11-25/server/tools

`python3 scripts/smoke_tracking.py` covers external links through both adapters.
`python3 scripts/smoke_interchange.py` covers MSPDI import/export parity, refused applies and round trips.
`python3 scripts/smoke_store.py` covers schema-version refusal, backups during writes, restore and verification.
`python3 scripts/smoke_agent.py` verifies real-process CLI/MCP query equality and shared execution,
progress reporting, revision conflict, evidence, blocker, decision and independent-verification behavior.
It includes `scripts/smoke_conditional.py`, which covers conditional-work parity and choice changes.

`ratify_contract` approves a complete Proposed task as a human/service; `reject_work` requires
Submitted work, a different reviewer, and a nonempty `reason`. Rejection retains ownership and
the latest review in `last_rejection`. Both require `key` and `base_revision`.
`explain_work.gates` and `project_status.gates` expose the same structured claim conditions;
`explain_work.transitions` adds start, submit and verify.

`workspace_list` and `workspace_register` manage device configuration through the shared registry.
Registration accepts `database` and optional `replace`; neither tool takes a project revision.
`workspace_list` reports each store's `store.status` and `shared_with`, and registration refuses a path
bound to another identity with `workspace_path_bound` (see [project selection](projects.md)).
Their results contain `local_config: true` and `data`, without a project operation or revision.
`attach_git_head` accepts an explicit `resource` key when the selected locator does not bind one.
