# QueryHive vs TablePro: UI/UX design audit

Read-only audit, 29 Sep 2026. I did not edit any file.

**Path prefixes used below**
- `QH/` = `/Users/isal/Workspaces/Lab/Experiments/query_hive/app/Sources/QueryHive/`
- `TP/` = `/Users/isal/Workspaces/Lab/Experiments/TablePro/TablePro/`

**Evidence legend**
- **[T] Traced:** I read it in source.
- **[S] Seen:** read from TablePro's own screenshot, `/Users/isal/Workspaces/Lab/Experiments/TablePro/.github/assets/app-dark.png`.
- **[I] Inferred:** derived from arithmetic, from grep hits or their absence, or from naming. Not confirmed on a render.

**Limits of this audit**
- QueryHive has no screenshots under `assets/` or `docs/`, and I had no shell to run `--snapshot`. Every QueryHive visual claim is therefore traced from code, not seen.
- The contrast ratios below are computed from source alpha values over the canvas hex. They are not measured pixels.

**License boundary.** TablePro is AGPL. Every recommendation here describes behaviour in QueryHive's own terms. Nothing names TablePro code to port.

---

## Verdict

QueryHive has a stronger visual identity, and its data copy is more honest. TablePro is far ahead on platform nativeness, keyboard reach and accessibility.

**Where TablePro wins.** It is an AppKit-first shell [T]:
- `NSSplitViewController` with sidebar, detail and inspector: `TP/Core/Services/Infrastructure/MainSplitViewController.swift`.
- A customizable `NSToolbar` with overflow validation and shortcut tooltips: `TP/Core/Services/Infrastructure/MainWindowToolbar.swift`.
- Window title, subtitle and proxy icon.
- `NSOutlineView` trees and an `NSTableView` grid with a full keyboard cell cursor: `TP/Views/Results/KeyHandlingTableView.swift:399-564`.
- Per-cell VoiceOver elements over drawn cells: `TP/Views/Results/Cells/DataGridCellAccessibilityView.swift`.
- A VoiceOver rotor for query issues: `TP/Views/Editor/QueryDiagnosticsRotorSearch.swift`.
- Reduce Transparency, Increase Contrast and Reduce Motion handled centrally: `TP/Theme/MaterialAccessibility.swift` and `TP/Theme/MotionAccessibility.swift`.
- Localized strings throughout.

**Where QueryHive stands.** It is a hand-drawn SwiftUI shell [T]:
- No `NSToolbar`, no `NSSplitView`, and no drag-and-drop anywhere (grep: zero hits in `QH/`).
- 15 accessibility modifiers in the whole app, all in Settings, one in the connection colour picker. TablePro has accessibility modifiers in 215 files.
- Zero reduce-motion, transparency or contrast checks.
- Zero localized strings.
- No keyboard path into the result grid or the object tree.

**Where QueryHive is ahead.**
- Its categorical colour vocabulary is protected from the accent: `QH/Support/Theme.swift:13-26`.
- Syntax colours are contrast-measured on light themes (`app/DESIGN.md` §Appearance).
- The grid is honest about the difference between in-memory and server work: sort banner, "First N rows · limit reached", Count all, and "No rows match… Clear Filters".
- The `--snapshot` scene harness has 60+ scenes (`QH/Support/Snapshot.swift`), a design-QA advantage TablePro has no equivalent for.

**The planned rewrite is the moment to fix most of this.** The grid is moving to `NSTableView` with drawn cells and the editor to incremental highlighting. Both rewrites must include keyboard and VoiceOver work from day one. TablePro's own source documents the trap: drawn `NSTableView` cells read as a grid of blanks to VoiceOver unless each cell gets an accessibility element [T].

---

## Per-surface assessment

### 1. Window layout, sidebar, tabs, toolbar

**TablePro [T][S]**
- `NSSplitViewController` with sidebar, detail and inspector, so Toggle Sidebar works through the responder chain.
- `NSToolbar` with `allowsUserCustomization`, autosaved configuration, centred connection and database items, back and forward navigation, a Safe Mode control, and Quick Switcher, SQL preview and history items (`MainWindowToolbar.swift:108-115,381-410`).
- The tab strip is a titlebar accessory. Tabs can be reordered and torn off into new windows (`MainSplitViewController.swift:121-139`; `TP/Views/Main/EditorTabReorderCancelMonitor.swift`).
- Minimum window size is 720×480 (`TabWindowController.swift:181`).
- The screenshot shows macOS 26 glass toolbar capsules, an inset sidebar, and equal-width Safari-style tabs.

**QueryHive [T]**
- Hand-drawn shell in `QH/Views/RootView.swift`: an empty `TitleStrip`, an `HStack` with a custom drag `SidebarResizer`, and a custom `StatusBar`.
- `windowStyle(.hiddenTitleBar)` and minimum size 1120×700 (`QH/App.swift:43-48`).
- Tabs are `TabChip`s selected by `onTapGesture`, not buttons. The close button stays in the accessibility tree while invisible (`opacity(0)`). The run state is a 6pt dot whose colour is the only signal (`QH/Views/Workspace.swift:111-165`).
- No tab reordering and no next/previous-tab or focus-pane shortcuts. `ShortcutAction` has 14 actions and none of those (`QH/Models/Shortcuts.swift:8-21`).
- The seams can only be resized by pointer drag.

**Status: behind.** QueryHive is ahead only on DESIGN.md's discipline about why each control sits where it does.

**Recommendation**
- Keep the CleanMyMac surfaces, but host them in native structure:
  - `NSSplitViewController`, or `NavigationSplitView` if it holds up, for sidebar and workspace, so ⌃⌘S, the sidebar tracking separator and keyboard-resizable dividers come free.
  - An `NSToolbar`, or SwiftUI `.toolbar`, carrying the context breadcrumb and Run. That buys overflow handling and, on macOS 26, system glass without redrawing it.
- Make tabs `Button`s:
  - add the `.isSelected` trait;
  - set the accessibility value to the stage ("running", "failed");
  - give the stage a shape or glyph as well as a colour (running spinner, failed exclamation);
  - add ⌘1–9, ⌃Tab/⌃⇧Tab and ⌘⇧[ / ] to `ShortcutAction`.
- Re-measure the 1120pt minimum. It was set for the toolbar that no longer exists (DESIGN.md §One toolbar).

**Priority:** P0 for accessible tab chips and tab shortcuts. P1 for the native split and toolbar.

### 2. Connection list and form

**TablePro [T]**
- A separate Welcome window whose source list is an `NSOutlineView` (`TP/Views/Welcome/*`).
- The connection form is its own window with four tabs: General, Network, Options, Appearance.
- Each tab reports its validation problems in words ("which field is empty") rather than as a bare disabled Save (`TP/Views/ConnectionForm/ConnectionFormTab.swift:43-71`).

**QueryHive [T]**
- A 620×660 sheet with three steps: type tiles, then URI, then form (`QH/Views/ConnectionsViews.swift:108-255`).
- Brand-mark driver tiles.
- The Test result sits beside the button.
- `missingRequired` feeds field outlines (`field(invalid:)`) but produces no sentence.
- Test Connection is bound to ⌘T inside the sheet (`:607`), the same key New Query uses in the QueryHive scheme.

**Status: on par.** The tile-first flow and inline test result are good.

**Recommendation**
- Name the missing fields beside Save, e.g. "Host and User are required". The data is already in `missingRequired`.
- Rebind Test to ⌘↩ or leave it unbound. ⌘T should mean one thing app-wide.

**Priority:** P1.

### 3. Schema tree

**TablePro [T]**
- `NSOutlineView` (`TP/Views/Sidebar/DatabaseTreeOutlineView.swift`) with a native search field, in-place rename, favourites and routines/triggers/types rows.
- Row size follows the system sidebar size (`SidebarRowSizePreference`).
- Truncate and delete are staged as pending (`SidebarView.swift:23-24`).

**QueryHive [T]**
- A recursive `LazyVStack` of `TreeRow`s that are only tap-selectable (`QH/Views/SidebarTree.swift:270-321`). No arrow-key navigation, no type-to-select, no focus ring.
- The disclosure chevron is an 11×11pt target.
- Level colour icons: catalog violet, database ice, schema amber.
- The tree filter only searches what is loaded, and the placeholder says so.
- Truncate and Drop go through the Safe Mode contract.

**Status: behind** on input and accessibility. **Ahead** on honesty (loaded-only filter, the "Empty" tray row versus a spinner).

**Recommendation**
- Rebuild as `NSOutlineView` with the same row visuals: 23pt rows, 15pt indent, driver tile on connection rows, level tints.
- That gives, for free: ←/→ to collapse and expand, type-select, VoiceOver outline semantics, Return to open, and system sidebar sizing.
- Until then, at minimum, make rows focusable and handle arrow keys and Return.

**Priority:** P0 for keyboard reach. P1 for the `NSOutlineView` rebuild.

### 4. SQL editor (rewrite target)

**TablePro [T]**
- Tree-sitter-backed editor (`TableProEditorKit`/`TableProGrammars` imports in `TP/Views/Components/SQLReviewSheet.swift:8-9`).
- Theme covers editor colours for current line, current statement, invisibles and 8 syntax tokens (`TP/Theme/ThemeColors.swift:68-89`).
- Font family and size are both configurable (`TP/Theme/ThemeFonts.swift`).
- Diagnostics are exposed as a VoiceOver custom rotor.
- Vim mode indicator; on-disk change banner.

**QueryHive [T]**
- `NSTextView` with TextKit 1 (`QH/Views/SQLEditor.swift:35-135`) and all smart substitutions off.
- Custom `LineNumberRulerView` with run markers (11×8, accent) and fold markers (`:1294-1321`).
- Current line and statement bands drawn behind the text (`:978-997`).
- Find bar above the text.
- Keyboard-only suggestion popup.
- Font family is configurable but the size is fixed at 13 (`:44`).
- The gutter "lift" is white at 3.5%, which the code's own comment calls invisible on light themes (`:1223-1236`).
- No diagnostics or squiggles, no bracket matching (grep), no accessibility rotor.

**Status: on par** for daily editing. **Behind** on diagnostics and accessibility. **Ahead** on light-theme syntax contrast (measured, per DESIGN.md).

**Rewrite recommendations**
- Incremental highlighting per edited paragraph and statement.
- Diagnostics drawn as underlines, each with a VoiceOver rotor entry ("Query issues") and a status readout.
- Bracket and quote matching.
- An editor font-size setting (⌘+ / ⌘−).
- Flip the gutter to a *recess* on light canvases.

**Visual-parity requirement.** The rewrite must pixel-match a `--scene syntax` / `suggest` / `fresh-tab` snapshot diff on:
- per-appearance token colours (each ≥4.5:1 except `comment`);
- caret and default text in `labelColor`;
- gutter hairline `ink 0.07`;
- numbers in `Tone.readout` (0.55);
- the 14pt run column and marker geometry;
- line and statement bands full-width behind text, with the `HighlightBand` inset fix;
- placeholder aligned via `textOriginX`;
- the corner line-count readout;
- the clear-button overlay;
- the find bar laid out above, never over, the text.

**Priority:** P0 that the rewrite ships the parity diff and at least the rotor/AX plumbing. P1 for diagnostics UI and font size.

### 5. Results grid (rewrite target)

**TablePro [T][S]**
- `NSTableView` with drawn rows.
- A full keyboard model:
  - arrows move a cell cursor;
  - ⇧-arrows extend, ⌘⇧-arrows jump to the edge;
  - ↩ edits, Esc clears;
  - ⇧Space selects whole rows;
  - Space previews the foreign-key target;
  - Delete removes rows.
- Per-cell accessibility elements are mounted only when an assistive client attaches. The label reads "Row r, column c: value, pending change…" (`DataGridCellAccessibilityView.swift:105-128`).
- Staged-change colours: modified yellow, inserted green, deleted red (`ThemeColors.swift:146-158`).
- The screenshot shows FK arrow chips, enum steppers, and red/green/yellow row states.

**QueryHive [T]**
- `LazyVStack` inside a two-axis `ScrollView` (`QH/Views/ResultGrid.swift:286-299`).
- Selection is by `DragGesture` only (`:974-995`). There are no `onKeyPress` or `onMoveCommand` handlers in `ResultGrid.swift` (grep), so there is no keyboard cell navigation, no ↩-to-edit, and nothing for VoiceOver.
- Every cell carries a `.help(value)` tooltip (`:1033`).
- Widths are estimated from character counts (×7.2).
- Contrast on the midnight canvas [I]:

| Element | Source value | Contrast | Floor it misses |
|---|---|---|---|
| Row numbers | `ink 0.35` (`:1012`) | ~3.1:1 | 4.5:1 |
| NULL | `ink 0.3` (`:1036`) | ~2.6:1 | 4.5:1 |
| Filter funnel | `ink 0.30` (`:1118`) | ~2.6:1 | 3:1 for UI |

  On Daylight these are lower still: row numbers come out around 2.5:1.
- Commit and discard of staged edits are two unlabeled 22pt glyphs in the footer (`:1201-1206`). Undo is menu-only, not wired to ⌘Z (`:536-542`).
- Strengths:
  - type chips tinted by category;
  - sort banner explaining in-memory vs server sort, plus the server escalation (`:556-577`);
  - honest footer ("First N rows · limit reached", Count all);
  - three distinct empty sentences with a Clear action (`:210-250`);
  - staged edit shown by amber wash *plus* a dot, so not colour alone.

**Status: behind** on input, accessibility and scale. **Ahead** on honesty of state.

**Rewrite recommendations (must-haves)**
1. A cell-cursor keyboard model:
   - arrows, ⇧/⌘⇧ extension, ↩ edit, Esc, Tab/⇧Tab move;
   - ⌘C / ⌘⇧C copy with headers;
   - Space opens `CellValueViewer` (see §HIG extras).
2. VoiceOver:
   - every cell exposes its row, column, header name, value and staged state;
   - the header exposes its sort state;
   - "N × M selected" is announced when a selection settles.
3. Contrast floor: row numbers and NULL ≥4.5:1, funnel and separators ≥3:1. That means roughly `ink 0.55` minimum on dark and higher on light, plus an Increase Contrast variant.
4. Staged-change grammar: amber = modified (keep), plus inserted and deleted states, each with a non-colour mark (dot / plus / strikethrough).
5. Commit becomes a labeled `PillButton` ("Review 3 Changes…") with ⌘S, and undo is wired to the window's undo manager.
6. Drop per-cell tooltips in favour of the inspector or a hover-delay expansion.

**Visual-parity requirement.** Diff against `--scene grid`, `grid-selection`, `grid-edits`, `grid-sorted`, `grid-columns`, `grid-inspector`, `grid-empty`, `grid-loading`, `grid-filtered-out`, `explain`. Parity covers:
- `#` gutter shared by header and rows, removed entirely when off;
- two-line header (name at 12 semibold mono, then the type `Chip` tinted mint numeric / violet bool / amber date-time / ice other);
- rename dot and accent sort chevron;
- always-visible funnel;
- header band `recess 0.30` with an `ink 0.12` bottom hairline;
- numeric columns right-aligned;
- alternate band `ink 0.03` (toggle);
- column separators `ink 0.05`;
- selection `accent 0.20` covering cell padding;
- staged `amber 0.20` plus a 4pt dot;
- NULL italic dim with configurable text, and `∅` for an empty string;
- row heights from `DataPreferences.RowHeight`;
- width algorithm: header vs the first 200 rows, clamp 84–320 + 22, proportional slack, gutter subtracted;
- footer semantics, amber when truncated, filtered or searched;
- sort and server-sort banners;
- side inspector at 340pt.

Contrast values are the one deliberate *non*-parity item.

**Priority:** P0.

### 6. Cell viewer / row inspector

**TablePro [T]**
- `RowInspectorView` in the window's trailing split item.
- Fields and JSON renderings of the *whole row*, with search, "edited only", and field editors (multi-line, JSON, enum, date, FK picker) (`TP/Views/RowInspector/RowInspectorView.swift`; `TP/Views/Results/*Picker*`, `*Editor*`).

**QueryHive [T]**
- `CellValueViewer` reads one cell as Text, Tree or Hex, as a popover or a 340pt side panel. It is read-only and has a per-column display format (`QH/Views/CellValueViewer.swift:22-103`).
- A multi-cell selection shows "r × c cells chosen".
- There is no whole-row view.

**Status: behind** (no record view). **On par** for single-value reading, with clearly stated limits.

**Recommendation**
- Add a **Record** mode to the existing side panel: every column of the focused row as label/value pairs, with the type chip and a staged-edit mark. Clicking a field focuses that cell in the grid.
- Keep the Text/Tree/Hex reader as the expansion of one field.
- Keep editing in the grid for now; the side panel should not become a second editor yet.

**Priority:** P1.

### 7. Structure view

**TablePro [T]**
- `TP/Views/Structure/*`, about 40 files: an editable structure grid for columns, indexes, FKs and triggers, a DDL view, a create-table form, and empty states per section (`TP/Views/Components/EmptyStateView.swift:100-152`).
- The screenshot shows a Data | Structure switch leading the status bar.

**QueryHive [T]**
- `ObjectsPane` lists a schema's objects with driver-native columns.
- `ObjectInspector` shows the selected table's columns and types, read-only (`QH/Views/Workspace.swift:581-957`).
- No per-table structure tab; no indexes or DDL.

**Status: behind.** The adoption analysis already defers the editor.

**Recommendation**
- Add a read-only **Structure** view for a table tab, reached by a Data | Structure segmented control in the grid footer's leading edge. It shows columns (name, type chip, nullable, default), then indexes, then DDL in the editor's own highlighter.
- No editing.

**Priority:** P2. It is only a P1 if daily users ask "what's the DDL" often.

### 8. Filters

**TablePro [T]**
- A filter panel builds WHERE rows: an enable-all tristate, Match all/any, a "Preview Query" sheet showing the SQL, presets, and Apply on ⌘↩.
- Esc returns focus to the grid (`TP/Views/Filter/FilterPanelView.swift:24-117`).
- These are server filters.

**QueryHive [T]**
- Per-column funnel popover with two shapes: a value checklist up to 10 distinct values, otherwise a search box (`QH/Views/ResultGrid.swift:1106-1162`).
- Cross-column in-memory search with "Search Server" escalation (`:383-414`).
- Presets menu.
- Every surface states that it narrows fetched rows only.

**Status: on par**, with a different model.

- **Ahead:** the distinct-value picker, and the honesty about what is in memory versus on the server.
- **Behind:**
  - no multi-condition server WHERE builder;
  - no SQL preview of what the server search will run;
  - the funnel is about a 14pt target at 2.6:1.

**Recommendation**
- Keep the funnel.
- Add "Show SQL" to Search Server and server sort, reusing the change-review sheet.
- Enlarge the funnel's hit area to the header's full height at the trailing 20pt.

**Priority:** P1.

### 9. Settings

**TablePro [T]** (per `docs/architecture/tablepro-settings-study.md`)
- `NSTabViewController` with `tabStyle = .toolbar`, 12 panes, a theme editor, and a keyboard rebinding recorder with conflict detection.

**QueryHive [T]**
- A custom icon-over-label pane bar drawn as `Button`s with the `.isSelected` trait, which is good (`QH/Views/SettingsView.swift:143-189`).
- A fixed 560×640 window.
- Cards with `HelpHint`, which puts the explanation in a hover tooltip.
- The window title does not follow the active pane [I]. A native toolbar-style settings window titles itself by pane.

**Status: on par** for the scope QueryHive has chosen.

**Recommendation**
- Set the window title to the pane name.
- Give each `HelpHint` a keyboard- and VoiceOver-reachable equivalent (an `.accessibilityHint`, or a click that opens a popover). Hover-only prose is invisible to keyboard users.
- Add editor and grid **font size** here.

**Priority:** P1.

### 10. Welcome and empty states

**TablePro [T]**
- A Welcome window: app icon, version, update link, "New Connection…" (prominent), Open File, and an Import menu (`TP/Views/Welcome/WelcomeActionsPanel.swift`).
- A shared `EmptyStateView`: icon, title, description, primary and secondary action, footer (`TP/Views/Components/EmptyStateView.swift`).

**QueryHive [T]**
- `EmptyWorkspace` is `HiveHero`, "No query open" and New Query (`QH/Views/Workspace.swift:58-75`).
- **Stale copy:** it says "Run streams the result straight to disk". Since the Run/Export split, Run writes nothing (DESIGN.md §Run looks, Export writes; `ResultGrid.swift:182`).
- The sidebar empty state says "Add a Trino coordinator" although three drivers ship (`SidebarTree.swift:128`).
- The editor placeholder hard-codes `hive.analytics.penerima_manfaat` (`Workspace.swift:359`).
- `HiveHero` bobs forever with no Reduce Motion check (`Theme.swift:853-857`).

**Status: on par** on concept, with three copy bugs that are cheap to fix.

**Recommendation**
- Fix the three strings now.
- Give `EmptyWorkspace` the recent connections and favourites as one-click rows. That is QueryHive's own welcome, without a second window.
- Gate the bob animation on Reduce Motion.

**Priority:** P0 for copy and motion. P1 for recents.

### 11. Dialogs and confirmations

**TablePro [T]**
- `SQLReviewSheet` shows statements verbatim for a confirmation, has a destructive style, and only takes the default Return action when the user asked for the dialog, so a keystroke already travelling to the editor cannot confirm it (`SQLReviewSheet.swift:13-57`).
- `DialogFooter` groups Cancel and the default action at the trailing edge (`TP/Views/Components/DialogFooter.swift`).
- `AlertHelper.confirmDestructive` is used throughout.

**QueryHive [T]**
- `RunConfirmationSheet`, the Safe Mode `confirm` prompt, has no `.keyboardShortcut(.cancelAction)` or `.defaultAction`. Its approve button is the accent `HubButton`, the same styling as Run (`QH/Views/RunConfirmationSheet.swift:51-56`).
- Connection delete uses a native `confirmationDialog` with the destructive role (`RootView.swift:50-62`), which is good.

**Status: behind** on the one dialog that guards writes.

**Recommendation**
- Esc should cancel.
- Approve should use a destructive `coral` treatment, a named verb ("Run 2 Writes"), and **no** Return default when the prompt was raised by the engine rather than the user.
- Apply the same rule to Truncate and Drop.

**Priority:** P0.

### 12. Notifications and errors

**TablePro [T]**
- `InlineErrorBanner` above results: selectable text, Copy and Dismiss buttons with accessibility labels, and a scroll cap (`TP/Views/Results/InlineErrorBanner.swift`).
- `UserNotifications` with per-operation thresholds (`TP/Core/Services/Notifications/*`).
- VoiceOver announcements on window and grid events (`TP/Extensions/AccessibilityAnnouncement.swift`).
- `LoadingReveal` holds back a spinner until a grace period passes, then shows it for at least a minimum time (`TP/Views/Components/LoadingReveal.swift`).

**QueryHive [T]**
- A preview error replaces the whole grid body with centred coral text. It is not selectable and has no copy button (`ResultGrid.swift:177-178,187-201`).
- App notices are modal `.alert`s (`RootView.swift:63-70`).
- No system notifications (grep).
- The status bar shows a mini spinner and the summary.

**Status: behind.**

**Recommendation**
- Replace the error body with an inline banner above the (possibly still visible) previous result: selectable message, Copy, Dismiss, and "Show in Log".
- Send a system notification when an export or to_table run over about 20s finishes while the app is not frontmost. QueryHive's long streaming exports are exactly the use case.
- Add a VoiceOver announcement for Run finished, failed or cancelled.

**Priority:** P0 for a selectable, copyable error. P1 for notifications and announcements.

### 13. Theming and dark mode

**TablePro [T]**
- Data-driven themes: editor, grid, UI, sidebar and status colours. Unset values fall back to system colours (`TP/Theme/ThemeColors.swift`).
- A theme editor and registry.
- Materials drop to solid fills under Reduce Transparency or Increase Contrast (`TP/Theme/MaterialAccessibility.swift:20-43`).

**QueryHive [T]**
- Seven canvases in light/dark pairs, five accents, three tones, three presets.
- A dynamic `Tone.ink`/`Tone.recess` layer resolved per appearance by AppKit (`QH/Support/Theme.swift:79-127`).
- Categorical colours are fixed so a theme cannot collapse the vocabulary.
- `glass()` always uses `.ultraThinMaterial`, with no transparency or contrast fallback (`Theme.swift:246-256`).

**Status: ahead** on design rationale and on the protected vocabulary. **Behind** on accessibility variants. **Behind** on user themes, which QueryHive deliberately does not want.

**Recommendation**
- Route every material and glow through one modifier that reads `accessibilityReduceTransparency` and `colorSchemeContrast`:
  - under either setting, solid `canvas` or `recess` fills;
  - `ink` hairlines raised from 0.07 to about 0.20;
  - `secondary` raised from 0.68 to about 0.85;
  - Backdrop glows off.
- Tone `plain` is most of this already, so expose it as "the contrast variant" automatically.

**Priority:** P0.

### 14. Typography and density

**TablePro [T]**
- System text styles (`.caption2`, `.subheadline`, `.callout`).
- Editor and grid font family *and size* (`ThemeFonts.swift`).
- Four row heights: 20/24/28/32 (settings study §4).

**QueryHive [T]**
- Fixed point sizes throughout. 53 call sites draw text below 11pt, down to 9.5pt (grep over `.ui/.code(9…10.5)`), e.g. section titles at 9.5 bold (`Workspace.swift:940`, `SidebarTree.swift:171`).
- Font family choice exists; size does not.
- Three row heights (`QH/Support/DataPreferences.swift:15-30`).

**Status: behind.** The density is deliberate, but combined with 0.3–0.35 ink it drops below legibility floors.

**Recommendation**
- Set a floor of 11pt for anything a user must read, and 10pt only for decorative uppercase labels at ≥0.68 ink.
- Add editor and grid size settings.
- Scale `Metrics.treeRow` with the system sidebar size setting.

**Priority:** P1. Contrast is covered at P0 in §5 and §13.

### 15. Iconography

**TablePro [T]**
- SF Symbols plus about 34 driver brand image sets (`TP/Assets.xcassets/*-icon.imageset`).
- The TypeBadge exposes "Type: …" to VoiceOver (`TP/Views/Components/TypeBadge.swift:30`).

**QueryHive [T]**
- SF Symbols plus SVG driver marks generated to Swift (`QH/Support/DriverLogos.swift`), a hex mark system, and small- and large-size icon artwork.
- Icon-only buttons (`IconButton`) rely on `.help` for their name.

**Status: on par.** Stronger brand system.

**Recommendation**
- Give every `IconButton` and `Chip` an explicit `accessibilityLabel`: the type chip should read "type varchar", and the stop, explain, count and commit glyphs need labels. `.help` alone is not a reliable name.

**Priority:** P0. It is cheap and part of the accessibility floor.

### 16. Keyboard-first navigation and quick switcher

**TablePro [T]**
- `NSPanel` quick switcher with a HIG scope bar (segmented control), cross-connection results, recents, a key monitor, and increased-contrast dividers (`TP/Views/QuickSwitcher/QuickSwitcherPanelView.swift:64-160`).
- Rebindable shortcuts that are synced into the menus and toolbar tooltips.

**QueryHive [T]**
- Open Quickly is an overlay searching loaded tree nodes, saved queries and history, with ↑/↓/↩/Esc (`QH/Views/OpenQuickly.swift`).
- There is no `ScrollViewReader`, so arrowing past the visible rows moves the highlight off-screen [T by absence].
- Rows are not announced to VoiceOver.
- The only menus are Query plus File additions. No View menu: no toggle sidebar, toggle panel or focus pane (`QH/App.swift:60-114`).

**Status: behind.**

**Recommendation**
- Scroll the highlighted Open Quickly row into view.
- Add a scope control: All, Objects, Saved, History.
- Expose results as a list with a selected row.
- Add a **View** menu: Toggle Sidebar ⌃⌘S, Toggle Result Panel, Focus Editor / Grid / Tree (⌘1–3 style or ⌥⌘ variants), Next/Previous Tab.

**Priority:** P0 for the View menu and focus moves (the grid and tree are unreachable otherwise). P1 for scopes.

### 17. Accessibility: VoiceOver and contrast

Summary of the above. TablePro treats accessibility as architecture:
- AX elements for drawn cells;
- a rotor for diagnostics;
- announcements;
- solid-surface and reduced-motion policy.

QueryHive has:
- no keyboard path to the grid or tree;
- colour-only tab state;
- sub-4.5:1 text in the grid;
- no motion, transparency or contrast handling;
- no localization infrastructure.

**Status: clearly behind.** Recommendations are in §§1, 3, 5, 11, 13, 15 and 16.

**Priority:** P0 overall.

---

## HIG-native patterns neither app has (would put QueryHive ahead)

Absence was checked by grep in both trees [I]. These fit QueryHive's export-first identity.

1. **Finder and Dock progress for streaming exports.**
   - Publish an `NSProgress` per output file, so the file icon in Finder shows a progress bar while it streams.
   - Show a badge or progress on the Dock tile during long exports and `to_table` runs.
   - Neither app has either. It is a perfect fit for "writes without holding the result in memory". **P1**
2. **App Intents / Shortcuts on macOS.** "Run saved query → export CSV to folder" as an intent. It turns saved queries into automation without the MCP path. TablePro only mentions iOS Shortcuts in its docs; its Mac source has no `AppIntent`. **P2**
3. **Quick Look on Space in the grid.**
   - Space opens the focused cell's value (JSON, text, hex) in a Quick Look-style panel, like Finder.
   - TablePro binds Space to FK preview instead.
   - Pair with ⌘Y. **P1** as part of the grid keyboard model.
4. **Drag and drop:**
   - drag a table from the tree into the editor to insert its qualified name;
   - drop a `.sql` file on the window to open it;
   - drag a grid selection out as a CSV file promise to Finder or Numbers.
   QueryHive has no drag or drop at all. TablePro has it only in the grid and for row reordering. **P1**
5. **CoreSpotlight indexing of saved queries and favourites,** so ⌘Space finds "monthly penerima report" and opens it in QueryHive. **P2**
6. **Services menu,** "New QueryHive Query with Selection", from any text app. **P2**
7. **Share and Print of a result selection:** `NSSharingServicePicker` with CSV, and ⌘P for the grid. **P2**

---

## Prioritized list

**P0 (must)**
1. Grid rewrite:
   - keyboard cell model;
   - VoiceOver per cell and per header;
   - contrast floor (row numbers and NULL ≥4.5:1, funnel ≥3:1);
   - labeled commit with ⌘S and undo;
   - the visual-parity snapshot diff.
2. Editor rewrite ships with the parity diff and accessibility plumbing (text-view accessibility intact, rotor hook).
3. Show **Safe Mode and environment** in the working chrome. Today Safe Mode appears only in `ConnectionsViews.swift` and `ImportSheet.swift` (grep), so a read-only vs full connection looks identical in the toolbar, tabs and status bar. Put a level glyph plus label in the breadcrumb and status bar, and optionally tint by connection colour.
4. `RunConfirmationSheet`:
   - Esc cancels;
   - destructive styling;
   - no Return default on engine-raised prompts.
5. Accessibility floor:
   - tab chips as buttons with selected trait and stage shown by shape;
   - `accessibilityLabel` on every `IconButton` and `Chip`;
   - Reduce Motion gate (HiveHero, Backdrop);
   - Reduce Transparency and Increase Contrast fallback for `glass()`.
6. Keyboard reach:
   - a View menu with Toggle Sidebar, Toggle Panel, Focus Editor / Grid / Tree and next/previous tab;
   - tree arrow-key navigation.
7. Selectable, copyable inline error banner.
8. Stale copy: `EmptyWorkspace` text, the sidebar "Trino coordinator" line, and the hard-coded placeholder table.

**P1 (should)**
- Native shell: split view plus toolbar hosting the breadcrumb and Run; re-measure the minimum window size.
- `NSOutlineView` tree.
- Record mode in the side panel.
- Named validation messages in the connection form; resolve the ⌘T conflict.
- System notifications and VoiceOver announcements for long runs.
- Font-size settings and an 11pt text floor.
- Open Quickly scroll-to-highlight and scopes.
- Settings pane titles and keyboard-reachable help.
- Finder/Dock export progress.
- Drag and drop.
- Quick Look on Space.
- Localization groundwork: `String(localized:)`, currently zero call sites.

**P2 (later)**
- Read-only Structure view (columns, indexes, DDL).
- Recents on the empty workspace (P1 if cheap).
- App Intents, Spotlight, Services, Share/Print.
- Tab reordering and tear-off; preview tabs.

---

## Where QueryHive is ahead (keep these through the rewrites)

- The protected categorical colour set, and "a theme may repaint the chrome, not collapse a vocabulary" (`QH/Support/Theme.swift:13-26`).
- Light-theme syntax palette with per-token contrast (DESIGN.md §Appearance).
- Honest grid states:
  - "First N rows · limit reached" in amber;
  - Count all as a deliberate second query;
  - sort banner explaining in-memory vs server sort, with server escalation;
  - three distinct empty-body sentences with Clear.
- A column filter whose shape is chosen by the data (value picker up to 10 distinct values).
- A driver-aware context breadcrumb with fixed-width levels.
- Explain rendered into the same grid.
- A 60+ scene `--snapshot` design-review harness. Use it as the parity gate for both rewrites.

## Not read

- TablePro:
  - `EditorTabStrip` internals;
  - `TableStructureView` bodies;
  - `SettingsWindowController` (taken from the settings study);
  - `DataGridRowView` drawing code;
  - editor internals beyond the rotor and theme.
- QueryHive:
  - `Panels.swift` bodies beyond struct names;
  - `ImportSheet.swift`;
  - `ContextCascade.swift`;
  - `SQLFindBar.swift` in full.

None of the findings above depend on these files.