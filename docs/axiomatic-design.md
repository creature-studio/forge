# Forge: axiomatic design and independence contracts

This document defines the application-layer decomposition used when changing
Forge. It is a design contract, not a numerical measurement of the entire
implementation. The independent axiom concerns changes to design parameters
with other parameters and domain inputs held fixed. A real file write changing
Git status is a required domain consequence, not itself a design defect.

## Functional requirements and design parameters

| FR | Required capability | Primary DP / ownership |
|---|---|---|
| 1 | Identify and access the workspace and its file scope | FilesState, FileFilter, forge-fs |
| 2 | Read and change repository state | forge-git, ScmState |
| 3 | Edit a document and persist its contents | EditingState, text document service |
| 4 | Index and query workspace content | ToolsState search/index state, forge-fs |
| 5 | Run and retain independent terminal sessions | TerminalSessions, EmbeddedTerminal |
| 6 | Assign input to exactly one active owner | InputContext, modal precedence, input_router |
| 7 | Reveal and present a requested file/diff without stale handoffs | NavigationState, PreviewState |
| 8 | Arrange and resize the workbench | LayoutState, PanePlan, forge-ui |

The intended control contract, for this ordering, is shown below. X indicates a
permitted functional dependency, not a Rust import or an event subscription.
Inputs from the shared filesystem, Git repository and explicit user actions
are held fixed when judging changes to DPs.

| FR / DP | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
|---|---|---|---|---|---|---|---|---|
| 1 | X | | | | | | | |
| 2 | X | X | | | | | | |
| 3 | X | | X | | | | | |
| 4 | X | X | | X | | | | |
| 5 | X | | | | X | | | |
| 6 | | | X | X | X | X | | |
| 7 | X | X | X | X | | X | X | |
| 8 | X | X | X | X | X | X | X | X |

FR4 may use a Git status snapshot for status-filtered results. FR6 uses
immutable mode/focus inputs from active owners. FR7 combines explicit reveal
requests with document/SCM state. FR8 consumes presentation inputs; changing
geometry must not issue file or Git mutations.

The table is a **target**, not proof that every application path already
satisfies the independence axiom. In particular App still orchestrates shared
FilesState, ScmState, PreviewState, EditingState and ToolsState. Preview rendering
also receives viewport geometry; the semantic preview requirement and its
layout-dependent presentation must be distinguished. Prove independence at
the chosen FR granularity by perturbing a DP and checking other FR invariants.

## Remaining functional coupling (second review)

The original eight-row target is not the measured design matrix. In particular,
FR1's *status-filtered presentation* explicitly consumes DP2's snapshot;
explicit reveal consumes a path and changes the Files selection; FR6 uses the
Find state and the focused pane. FR7's screen-row representation depends on
DP8's viewport. These requirements must not be erased simply to obtain a
triangular drawing. Keep the workspace identity, document identity, source-line
anchor and underlying repository independent of presentation changes. Treat
user-requested activation/reveal and changed shared data as named coordination
inputs. A full quantitative FR-DP independence assessment is still open.

## Enforcement in code

- TerminalSessions owns tab lifecycle and private inactive sessions. Only the
  active PTY receives input; hidden tabs remain alive. EmbeddedTerminal owns
  its visibility and focus setters. Workspace pane focus is the saved return
  target, not a competing PTY focus flag.
- LayoutState owns size preferences and computes an immutable PanePlan. Its
  resize methods do not have access to App, Git, file I/O or PTY sessions.
- InputContext resolves key ownership from a snapshot, while App adapts actual
  state to that context. Keyboard and mouse share modal ownership. Ordinary
  keys, paste and terminal cursor use the same typing owner. Explicit pointer
  handoffs make Find passive when a PTY is focused, or release the PTY when
  Find is focused; neither destroys the other state. A Terminal activity click
  is a direct command rather than a synthesized key routed through Find.
  Workspace modal handlers share the same routing entry points; the commit
  composer remains component-local.
- FileFilter derives a shared file/search scope from explicit, read-only Git status.
  ToolsState owns cancellation, generation invalidation and index/context cleanup.
  Accepted Git snapshots rebuild the filtered tree and invalidate search only
  when the effective allowed-path set changes. Canceled, stale or equivalent
  snapshots do not invalidate search. Losing the repository clears the active
  status-filtered scope.
- FilesState owns tree rebuilding, explicit reveal filtering, selection restoration,
  compact expansion and selection movement. Rebuilds use one filter snapshot
  and an explicit viewport height, without requiring App or I/O.
- PreviewState owns content acceptance, refresh scroll preservation, source-line
  reveal and diff cache installation. The workbench supplies an explicit
  document identity, not a live file-tree selection predicate, and handles
  the resulting search/focus effects. PreviewState also owns its
  frame preparation, content mapping, presentation caches, scroll clamping
  and selection capture. It accepts explicit viewport/theme inputs and returns
  geometry; it cannot issue workspace/Git mutations. Code reflow preserves
  the semantic source-line anchor where viewport bounds allow. Formatted
  Markdown is not assigned a fictitious source mapping.
- EditingState owns asynchronous load/save completion and dirty-state transitions.
  Successful saves report a typed result; App coordinates external refreshes.
- ScmState owns Git sidebar row selection, ancestor expansion and asynchronous
  stage/unstage transfer reconciliation. It accepts a borrowed history view
  and compact-folder preference, not App or file/terminal state.
- NavigationState owns an atomic path/line/focus request and per-worktree
  navigation snapshots. Unrelated completions cannot consume that request;
  workspace switches clear pending navigation. An explicit file selection
  cancels a previous reveal. FilesState owns the ancestor expansion and
  selection operations used by reveal. Navigation supplies path and viewport,
  not mutations of tree fields. Sidebar visibility returns an explicit
  transition. App coordinates the explicit focus handoff; changing sidebar
  visibility does not request a Git snapshot. Git activity entry is a user
  command and can request a refresh independently of visibility.
- WorkspaceChange is the boundary for successful file saves, Git writes,
  tools writes and watcher notifications. App's coordinator applies the
  refresh actions. A pure refresh planner uses only an immutable presentation
  snapshot, and the deferred queue owns its merge/drain lifecycle. Deferred
  notifications are coalesced while writes are
  pending, bounded to 512 distinct paths, and flushed after job completion.
  Workspace changes discard deferred notifications from the previous root.

### Compile-time state ownership (third review)

FilesState, ScmState and PreviewState are declared inside their owning
modules instead of the App parent module. Rust privacy now prevents sibling
modules from writing file refresh anchors, reveal overrides, compact-folder
continuations, Git transfer requests and deferred refresh flags, and the
split-diff render cache/row count. NavigationState's pending diff resume is
also private; its one-shot matching/consumption is owned by NavigationState.
WorkspaceNavigation composes opaque FileNavigation and ScmNavigation
snapshots. Each owner restores its own navigation state, clears transient
click/transfer state, and preserves the scheduler handoff needed to finish
in-flight status reads safely. A subset of presentation and job fields remains
visible to App; this is an incremental compile-time boundary, not a claim of
complete encapsulation or an empirically measured triangular matrix.

### Raw data, document identity, and layout independence (fourth review)

The old FR1 conflates filesystem access with Git-aware filtering. Split it
into the raw listing requirement and the projected file-list requirement;
removing Git-aware filtering is not an acceptable way to eliminate a matrix
entry. For the following *control submatrix*, hold explicit user commands,
shared filesystem contents, and repository inputs fixed. A blank denotes an
intended absence of control dependency, not universal measured zero:

| FR / DP | Raw listing | Git facts | File projection | Document state | Layout |
|---|---|---|---|---|---|
| Raw listing | X | | | | |
| Git facts | X | X | | | |
| Filtered file projection | X | X | X | | |
| Open document identity/content | X | X | X | X | |
| Display geometry | X | X | X | X | X |

This ordering is a refinement of distinct capabilities, not a relabeling of
an unchanged coupled matrix. The code changes supporting it are:

- FilesState owns a private workspace root and raw directory cache. A listing
  for an out-of-workspace path is rejected. A Git snapshot changes the
  projection only, not raw listings or workspace identity.
- Explicit workspace initialization is separate from ordinary refresh. Empty
  filtered results are not a workspace transition. A document opens by an
  explicit path/kind request; its current read handle, identity and page
  offset determine result acceptance. Passive file-list changes do not open
  another document. The active document has a separate shallow watch target
  even when its directory is collapsed or excluded from the projection.
- Sidebar geometry/visibility no longer schedules Git reads. Watch events,
  explicit refresh/activity commands and bounded periodic status reconciliation
  own freshness. For known repositories the periodic check has a 15-second
  minimum interval after completion and never queues an overlapping periodic
  read. Equivalent periodic snapshots do not reload a Diff; other snapshots
  retain the selected change identity where possible. The polling cost on
  very large repositories has not yet been benchmarked. A status snapshot is
  not a full diff-content fingerprint: watched document/index events and
  explicit refresh remain responsible for same-status content changes.

Direct regression evidence: Git scope changes preserve raw listings and the
open document; filtering while a document loads and subsequent refresh/paging
preserve that document; changing sidebar visibility schedules no status scan;
periodic reconciliation detects an external edit even while hidden and keeps
unrelated Diff position; document-watch filtering is path-scoped. The original
eight-row matrix still contains necessary status-filtering and explicit reveal
interactions. Focus/Find, pointer hit testing, and viewport-dependent rendering
also need their own FR definitions and perturbations. This five-row contract
is not proof that the full eight-row functional matrix is triangular.

### Mouse hit-testing boundary (fifth review)

The mouse_hit module reads the existing last-frame PaneLayout through a
borrowed HitGeometry view, and returns typed pane/header/Find targets,
overview coordinates, scrollbar candidates, and terminal body/tab regions.
ListHitGeometry maps visible/sticky rows to their original indices using
only the rectangle, scroll offset, borrowed sticky indices and row count.
Hit testing does not execute commands, schedule jobs, copy file/Git buffers,
or introduce another geometry cache. Input dispatch retains modal precedence,
activity press/release semantics and the existing gesture capture owners.
The scrollbar path now checks its painted lane before constructing the
scrollbar model, avoiding unnecessary SCM row generation or preview length
calculation for ordinary clicks. No new per-frame work is introduced.

This establishes a read-only geometry-to-input boundary for the migrated
workspace hit targets. Geometry is still a necessary input to mouse routing;
modal-specific controls, inline diff, splitter and drag handlers keep their
own capture/hit logic. The rendered row indices still rely on the current
list projection. Full input/layout independence and performance improvements
must not be inferred solely from this extraction; runtime CPU/latency and
cross-platform measurements remain outstanding.

### Pointer ownership, SCM projection and preview presentation (sixth review)

- Input routing has one typed handoff for Workspace, Find and Terminal. Modal
  precedence is unchanged. The existing Find and PTY focus fields remain the
  component projections, not a second independently stored focus owner.
- The pointer router resolves active gesture ownership from the component
  states before fresh hit testing. Preview selection and scrollbar drag now
  receive their continuation/release even over a PTY or an overview ruler.
  A new press supersedes a lost release; hiding a terminal cancels its capture
  and sends a native release to a child when needed. Modal controls retain
  their local gesture behavior. No additional global capture flag is stored.
- ScmState owns private status and expansion inputs; HistoryState owns private
  history row inputs and their revision. The flattened row projection is
  cached by revision, loading state and compact-folder mode. Status replacement,
  workspace restore and expansion mutations invalidate it. An Arc slice is
  shared by input, selection, decorations and the renderer without copying
  paths or rebuilding the tree on scroll. A short cache-only Mutex satisfies
  ratatui-kit's Send + Sync requirement. No I/O occurs under that lock.
- PreviewPresentation groups screen offsets, wrapping, selection and render
  caches; PreviewNavigation owns source-line/byte jump intent and a refresh
  anchor. Same-file refresh and subsequent reflow restore the original source
  line and wrapped-row displacement where the new content permits. Document
  payload and I/O identity remain separate from these presentation fields.
- Synchronous terminal-event handlers use write_no_update: ratatui-kit 0.10.3
  already renders after dispatch. Background completion and PTY notifications
  retain reactive wakeups. Idle notification ticks skip no-op writes, empty
  mark sets skip row decoration work, and code view avoids redundant Markdown
  preparation and a second source-row cache check.

Representative tests cover cross-pane drag/release, lost-release replacement,
row-cache identity and invalidation, and source-line preservation across a
same-file refresh and viewport resize. This does not prove all FR-DP matrix
entries. No end-to-end CPU, memory or input-latency improvement is claimed
without a before/after benchmark. Formatted Markdown remains display-row
based; exact byte preservation for arbitrary code edits is not guaranteed.

## Regression gates

Run `cargo test --workspace` and `git diff --check`. For Linux/WSL, run
`bash scripts/check-axiomatic-design.sh` to build and execute the isolated
real-terminal suite as well. The script uses temporary configuration and
private tmux sockets; tmux is the test harness, not a Forge runtime dependency.

Relevant tests include:

- `input_owner_is_independent_of_unfocused_domain_state`
- `typing_owner_matches_ordinary_keys_for_every_focus_combination`
- `find_terminal_pointer_handoff_has_one_typing_owner`
- `modal_mouse_cannot_focus_find_behind_it`
- `independent_splitters_do_not_change_each_others_preferences`
- `pane_plan_uses_only_geometry_and_focus_inputs`
- `newer_reveal_owns_line_and_focus_and_ignores_old_completion`
- `workspace_change_clears_pending_navigation`
- `workspace_restore_owns_selection_and_clears_transient_gestures`
- `worktree_restore_preserves_navigation_not_status_or_pending_transfer`
- `tabs_keep_panel_maximization_and_background_sessions_unfocused`
- `embedded_terminal_tabs_keep_sessions_and_close_independently`
- `saved_file_invalidates_search_index_without_watcher`
- `git_status_is_an_explicit_optional_filter_input`
- `layout_and_terminal_perturbation_preserves_file_git_and_search_state`
- `search_cache_invalidation_preserves_workspace_git_layout_and_preview`
- `source_line_is_preserved_across_width_and_wrap_changes`
- `reveal_expands_ancestors_and_selects_path_without_navigation_or_app`
- `git_scope_changes_are_the_only_status_driven_search_invalidation`
- `git_scope_changes_do_not_modify_raw_file_tree_or_open_document`
- `empty_or_changed_file_projection_cannot_reset_open_document`
- `sidebar_and_periodic_git_status_do_not_reset_unrelated_diff`
- `open_document_watch_is_path_scoped_not_tree_scoped`
- `filtering_and_selection_are_independent_of_app_and_io`
- `refresh_preserves_scroll_but_new_document_resets_it_without_app`
- `unrelated_file_and_diff_mode_reject_late_results`
- `save_completion_owns_dirty_and_close_state_without_app`
- `transfer_tracks_path_not_row_without_app`
- `stale_snapshot_retains_transfer_until_final_refresh`
- `save_policy_is_independent_of_history_and_existing_diff`
- `git_and_tool_refresh_policy_uses_only_relevant_inputs`
- `deferred_changes_remain_owned_until_writes_complete`
- `external_notifications_merge_paths_and_metadata_without_unbounded_growth`
- `unrelated_external_change_is_not_lost_during_local_write`
- Existing preview preservation, stale completion, modal key routing, Git
  refresh, scrollbar and worktree isolation tests.

New features should identify their FR, primary DP, required domain events and
state owner before adding code. Prefer adding a typed event or narrow method
over reaching into another feature's mutable state. Do not add a second
refresh or focus path as a fallback.

The information axiom is not quantified here: success probabilities and
acceptable tolerances have not been measured. Cross-platform real-terminal
interaction still needs platform-specific acceptance tests.


## Acceptance evidence (2026-11-27)

The application-layer refactor and the following representative independence
checks were exercised on the Linux/WSL development node. The tests use
disposable repositories and PTYs.

| Perturbed DP | Observed invariants / integration evidence |
|---|---|
| Workspace/filter | Filtering preserves path identity; explicit reveal and compact ancestry remain available. |
| Git | Staging and unstaging follow the selected path; another staged file is untouched. |
| Editor | Save completion owns dirty/close state; undo and redo save the expected bytes without staging files. |
| Search | Cache invalidation preserves Git, workspace, preview scroll and layout; external changes update filename/content search. |
| Terminal | Two real shell sessions retain independent variables across switches; maximize and hide/show preserve the active session; Ctrl+C interrupts a child command. |
| Input | Modal keyboard ownership does not steal or hide the underlying PTY; mouse selection sends the selected text to the clipboard in Markdown, code and Diff. |
| Navigation/preview | Stale requests are rejected; deep file reveal works in wide and narrow layouts; refresh preserves same-document position. |
| Layout | Splitter preferences are independent; resizing in an assembled App preserves workspace, Git status, search query/index and preview. |

Full workspace test run: 507 passed, 0 failed, 2 explicitly ignored performance
fixtures. Debug build passed. The six current real-terminal smoke scripts
listed in the regression gate passed. Legacy raw-PTY scripts
(`smoke-editor.py`, `smoke-search.py`, `smoke-auto-refresh.py`) failed
startup/terminal-negotiation assertions in this run; the new combined
tmux-backed smoke covers their core editor, search and watcher behaviors
but does not assert every legacy protocol/large-paste/regex scenario.
Several outdated smoke assertions were updated for the current keyboard
bindings, sidebar geometry and actual clipboard contents.

This is scoped engineering acceptance, not a proof that all 28 forbidden
upper-triangle entries are zero for every possible DP perturbation. App still
coordinates several mutable domain states, and live Codex behavior, Windows,
macOS, and the information axiom remain outside the verified scope.

### Second-review regression evidence

The subsequent preview/navigation/sidebar/scope boundary changes were checked
by `bash scripts/check-axiomatic-design.sh`: 510 passed, 0 failed, 2 ignored;
debug build and all six real-terminal smoke scripts passed. The earlier raw-PTY
protocol failures were not re-exercised by this gate. Source-line invariance
under code reflow, reveal ownership, and status-scope change gating now have
additional direct tests. The original full-matrix and platform limitations
remain; this evidence is not a mathematical proof of strict triangularity.

### Fourth-review regression evidence

The raw listing, document identity, and status freshness changes passed the
unified gate: 519 tests passed, zero failed, two ignored performance fixtures;
debug build and all six isolated real-terminal smoke scripts passed. The
five-row control submatrix above has representative regression evidence, not
exhaustive FR-DP certification. The original full-matrix, large-repository
performance, legacy PTY, and cross-platform limitations remain.
