---
name: outflow
description: Where your money is and where it's going, read against your own history.
colors:
  ink-bg: "#0a0e14"
  panel: "#111823"
  panel-recessed: "#0e141d"
  hairline: "#1e2a38"
  row-divider: "#172230"
  row-hover: "#0e1620"
  selected-fill: "#1c2c40"
  text: "#e6edf3"
  text-dim: "#8b98a9"
  text-faint: "#7a8696"
  field-teal: "#2dd4bf"
  teal-wash: "#0f2e29"
  teal-edge: "#1f5850"
  in-green: "#34d399"
  out-red: "#f87171"
  note-blue: "#60a5fa"
  attention-amber: "#fbbf24"
  kind-cash: "#7dd3c0"
  kind-credit: "#c4b5fd"
  kind-invest: "#f5d38a"
typography:
  wordmark:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica, Arial, sans-serif"
    fontSize: "23px"
    fontWeight: 700
    letterSpacing: "-0.5px"
  figure:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica, Arial, sans-serif"
    fontSize: "20px"
    fontWeight: 650
    fontFeature: "tnum"
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica, Arial, sans-serif"
    fontSize: "16px"
    fontWeight: 600
  row-name:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica, Arial, sans-serif"
    fontSize: "14px"
    fontWeight: 600
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica, Arial, sans-serif"
    fontSize: "13px"
    fontWeight: 400
  meta:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica, Arial, sans-serif"
    fontSize: "12px"
    fontWeight: 400
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, Roboto, Helvetica, Arial, sans-serif"
    fontSize: "11px"
    fontWeight: 600
    letterSpacing: "0.1em"
rounded:
  tag: "4px"
  control-inner: "6px"
  control: "8px"
  tile: "10px"
  card: "12px"
  pill: "999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "14px"
  lg: "16px"
  xl: "24px"
  page: "28px"
components:
  button:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
    typography: "{typography.body}"
  button-hover:
    backgroundColor: "{colors.row-hover}"
  button-key:
    backgroundColor: "{colors.teal-wash}"
    textColor: "{colors.field-teal}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  button-ghost:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text-dim}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  segmented:
    backgroundColor: "{colors.panel-recessed}"
    textColor: "{colors.text-dim}"
    rounded: "{rounded.control}"
    padding: "2px"
  segmented-on:
    backgroundColor: "{colors.selected-fill}"
    textColor: "{colors.text}"
    rounded: "{rounded.control-inner}"
    padding: "4px 9px"
  card:
    backgroundColor: "{colors.panel}"
    rounded: "{rounded.card}"
  stat-tile:
    backgroundColor: "{colors.panel-recessed}"
    rounded: "{rounded.tile}"
    padding: "12px 14px"
  nav-tab:
    textColor: "{colors.text-faint}"
    rounded: "{rounded.control}"
    padding: "5px 11px"
  nav-tab-on:
    backgroundColor: "{colors.selected-fill}"
    textColor: "{colors.text}"
  account-chip-cash:
    textColor: "{colors.kind-cash}"
    rounded: "{rounded.tag}"
    padding: "1px 6px"
---

# Design System: outflow

## Overview

**Creative North Star: "The Field Notebook"**

outflow records observations, not verdicts. The person's own history is the
baseline: a charge is "unusual" because it's unusual *for them*, never because
it broke a budget. The interface reads like a well-kept notebook opened at a
desk in the evening: a dark, calm page, dense rows of dated entries with
tabular figures, and an annotation in the margin only where something
deserves a second look.

Density is high and type is small, because the main reader sits at a desktop
and wants many rows in view. Depth comes from tonal steps between near-black
navy surfaces and hairline borders, not from shadows. Colour is an ink used
for meaning: money in, money out, the kind of account, and things that need
attention. The chrome around the data stays neutral so the figures carry the
page.

Controls are tactile and clear. They sit quietly at rest but always look
pressable: a visible border, a real hit area, and a distinct pressed and
focused state.

**Key Characteristics:**
- Dark, single theme (`color-scheme: dark`); near-black navy page with
  slightly lifted panels.
- System sans-serif throughout; all figures use tabular numerals.
- One interactive accent (Field Teal); every other hue encodes data meaning.
- Flat: hairline borders and tonal layering, no drop shadows on content.
- Narrow reading column (940px max) centred on the page.

## Colors

A cool, low-chroma navy neutral ramp with a small set of saturated inks that
each mean one thing.

### Primary
- **Field Teal** (field-teal): the one interactive accent. Wordmark, key
  action button text, the active-row marker, the net worth line. The washed
  key-button fill (teal-wash) and its edge (teal-edge) are the same ink
  diluted.

### Secondary
- **In Green** (in-green): money arriving and gains. Positive figures and the
  connected-status dot.
- **Out Red** (out-red): money leaving, losses and destructive headings.

### Tertiary
- **Attention Amber** (attention-amber): something needs a look. Pending
  states, the Settings badge, reconnect-needed connections.
- **Note Blue** (note-blue): informational tags and the "more" chip. Never a
  call to action.
- **Account-kind inks** (kind-cash, kind-credit, kind-invest): tint an
  account chip by what kind of money it holds. Cash is teal-green, credit is
  violet (loans too), investments are gold. Kind-invest also draws the
  Invested line on the Holdings chart.

### Neutral
- **Ink Background** (ink-bg): the page.
- **Panel** (panel): cards, buttons, the lifted surface.
- **Recessed Panel** (panel-recessed): stat tiles and segmented-control
  tracks, a step *below* the panel.
- **Hairline** (hairline): every border and divider between surfaces.
- **Row Divider / Row Hover / Selected Fill** (row-divider, row-hover,
  selected-fill): list row separators, hover wash, and the selected segment
  or active nav tab. In CSS these are `--lg-divider`, `--lg-hover`,
  `--lg-selected` and its edge `--lg-selected-edge`.
- **Text / Text Dim / Text Faint** (text, text-dim, text-faint): primary
  copy, secondary copy and labels, and metadata. Text Faint is the floor:
  it holds 4.5:1 (WCAG AA) on the page, panels and the hover fill, so any
  text may use it. Never dim text further with opacity.
- **Every colour is a token.** `ledger.css` has no hex value outside
  `:root`: washes and edges have their own tokens (`--lg-teal-wash`,
  `--lg-kind-cash-edge`, `--lg-blue-wash`, ...).

### Named Rules
**The Meaning-Only Rule.** Every hue other than Field Teal encodes data
meaning (in, out, account kind, attention). Chrome, layout and decoration
stay on the neutral ramp. If a colour doesn't answer "what kind of money, or
does this need me?", it shouldn't be there.

**The Not-Colour-Alone Rule.** In versus out is never told by green versus
red alone. The sign, a label or the lane always carries it as well.

## Typography

**Body Font:** the system sans-serif stack (-apple-system, BlinkMacSystemFont,
Segoe UI, Roboto, Helvetica, Arial).

**Character:** a single, unbranded native face that gets out of the way of
the numbers. Hierarchy comes from weight and size, not from a second family.

### Hierarchy
- **Wordmark** (700, 23px, -0.5px tracking): the product name in the header
  only.
- **Figure** (650, 20px, tabular): headline numbers in stat tiles: net
  worth, totals.
- **Title** (600, 16px): section headings on a screen (Connections,
  Card-payment review).
- **Row name** (600, 14px): the merchant or account name that leads a row.
- **Body** (400, 13px): table cells, explanations, button text.
- **Meta** (400, 11.5–12px): dates, sources, coverage lines, sync status.
- **Label** (600, 11px, uppercase, 0.06–0.1em tracking): card headers
  and stat-tile keys.

Nothing is set below 11px. Sizes are written in rem in `ledger.css`.

### Named Rules
**The Tabular Rule.** Every money figure uses `font-variant-numeric:
tabular-nums` and right-aligns in columns, so digits line up down the page.

## Layout

A single centred column, 940px max, with 24px side gutters and 28px top
padding. Screens stack full-width blocks vertically: a header row
(wordmark, then nav pushed right), an action bar, a coverage line, a strip
of stat tiles, then one or more cards. Gaps between blocks are 14–18px;
inside cards, rows pad 14px by 16px. Settings separates its sections with a
hairline rule and about 22–28px of space.

Below 640px, stat strips and chart pairs stack and row layouts wrap. The
phone is a glance surface; desktop is where the layout is tuned.

## Elevation & Depth

Flat by design. Depth is three tonal steps: Recessed Panel sits below Panel,
which sits on the Ink Background. Hairlines separate everything. Nothing
floats over the page but the toast, and it has no drop shadow either;
while one shows, the page gains room below its content so nothing has to
stay under it. A
selected figure or row is marked with the Selected Fill and a 2px Field
Teal inset ring rather than a lift.

### Named Rules
**The Hairline Rule.** Surfaces are separated by a 1px Hairline border or a
tonal step, never by a shadow.

## Shapes

Gently rounded rectangles throughout. Tags and account chips use 4–5px,
controls 8px (6px for a segment inside a track), stat tiles 10px, cards
12px. Pills (999px) appear only on badges and status capsules. Status dots
are circles. Nothing is sharp-cornered and nothing is fully rounded except
those capsules.

## Components

### Buttons
- **Shape:** gently rounded (8px).
- **Default:** Panel fill, Hairline border, Text colour, body size, 8px by
  14px padding.
- **Key:** the one primary action in a group (Pull). Teal-wash fill,
  teal-edge border, Field Teal semibold text.
- **Ghost:** the same shape with Text Dim text, for low-priority or
  destructive-but-guarded actions (Reset, Unlink).
- **Hover / Disabled:** hover lightens the fill one step; disabled drops to
  50% opacity with no pointer.

### Segmented controls
- A Recessed Panel track with a Hairline border and 2px inset. Options are
  bare text in Text Dim; the selected option gets the Selected Fill and Text
  colour. Used for the Holdings range (Month / 3M / YTD / 1Y / All) and
  its By account / By position lens.

### Cards / Containers
- **Corner Style:** 12px.
- **Background:** Panel, Hairline border, content clipped.
- **Header:** an uppercase Label heading with a bottom Hairline, controls
  pushed to the right.
- **Rows:** 14px by 16px padding, Row Divider between them, Row Hover
  on hover, and the inset teal bar when active.

### Stat tiles
- Recessed Panel, 10px radius, 12px by 14px padding. An uppercase Label
  key over a Figure value. A unit or qualifier sits beside the value in
  small Text Dim.

### Account chips
- Small (12px, 600) text in a 4px-radius outline chip, tinted by account
  kind: ink text, a dark edge of the same hue and a near-black wash. The
  last four digits stay in the label.

### Navigation
- Text tabs at 12px, pushed to the right of the wordmark. At rest they're
  Text Faint; hover goes to Text Dim; the current tab gets the Selected
  Fill, its edge and Text colour. Each tab is at least 28px tall. A count
  that needs attention sits in an Attention Amber capsule beside the label;
  screen readers hear it as words ("Settings, 2 items need a look").

### Recurring list (Flow)
- A table between the lanes and the transaction table: merchant (Row name,
  with cadence and category in Text Dim under it), a six-bar spark of the
  last six months, the change marker, usual cost per month (signed,
  tabular), and the last charge. Headers are sort buttons with `aria-sort`.
- The merchant button's hit area stretches over the whole row, so the row is
  a drill-down figure: the dotted underline sits on the name, and the
  selected row gets the Selected Fill and a 2px Field Teal inset ring.
- Only a change gets a margin note, in words: "new" and "up 44%" in
  Attention Amber, "down 12%" and "late" in Text Dim. Steady says nothing
  visible. There is no "down 100%": a merchant with no charge is late or
  steady.
- A merchant charged every few months reads "every ~2 months" under its
  name, and is compared charge to charge. Under that, a Text Dim year line
  ("$489.00 over 12 months, up 10% on the year before") shows once the
  archive covers two years: always for those slower merchants, otherwise
  only when the year moved 10% or more.
- The spark's bars are Text Faint (Text Dim on hover or selection); the
  month still in progress is an outlined bar, so a partial month doesn't
  read as a drop. A slower merchant's spark is its last six charges, one
  bar each, spaced a little wider (its months are empty by design); its
  accessible name lists the charges.
- Ignore is the row action, waiting for hover or focus like the table's.
  It's a deliberate exception to "upkeep stays off analysis screens": it
  only changes what this list shows, and it's undone in place (Undo on the
  toast, which covers several Ignores in a row, or Restore).
- Top eight first. Under the table, "Show all" on the left and a quieter
  "N ignored" text control on the right, which lists the ignored merchants
  with Restore, aligned with the table's cells.
- Focus and selection differ: selection is the 2px inset ring, focus an
  outline drawn inside it.
- Below 720px each merchant becomes two lines: name and cost, then spark,
  change, last charge and Ignore.

### Answer in words (Check-in)
- Check-in's focal moment: one sentence at 22px (450, money in 650,
  tabular) saying what the period did ("In October, net worth rose $309.88
  and you kept $172.51 of what you earned."), then a Text Dim tail that
  counts what needs you and what looks unusual. Nothing above it but the
  strip and the title bar, so the figures under it read as the evidence.
  Below 720px it drops to 19px.

### Check-in figures
- Three figures open on the page (no card), split by Hairlines like the
  Flow headline pair but at 24px: Net worth change, Earned − spent, Saved.
  Each has one Text Dim line of parts (Markets · Added, in · out, what
  stayed as cash) and one "usually" line with the usual in Text and 600.
  One Text Faint footnote says what "usually" means. In Month view each
  figure is a link (dotted Text Faint underline) to Holdings or Flow for
  that month; week and quarter figures are plain, since those screens
  don't show weeks or quarters. Below 720px they stack with Hairlines
  between.

### Unusual list (Check-in)
- One card of rows. Each row: the margin note in words ("reconnect",
  "review", "new", "up 44%", "large" in Attention Amber; "down 33%" and
  "late" in Text Dim: a skipped grooming is easy to ignore, a missing bill
  worth a glance), the name as a link with the dotted underline whose hit area
  covers the row, a Text Dim detail line saying what it's compared with,
  the signed amount when there is one, and the destination (Flow /
  Settings) in Text Faint. Rows open the destination already filtered:
  Flow drilled to the merchant (recurring window, or the charge's month
  for a large charge), Settings scrolled to the section.
- Checks that didn't run are a Text Faint list under the card, in words
  ("Recurring cost changes wait for a finished month.").
- Below 720px each row is note and destination, then name and detail with
  the amount at the right.

### Month strip (Flow, Holdings, Check-in)
- One shared component (`MonthStrip.tsx`, `lg-strip`): one button per
  month with the short month and a short figure. Flow adds a 20px bar (Out,
  scaled to the largest month, from zero: months that spent about the same
  look about the same, because they did; the exact figure is on the button,
  so no cropped baseline to exaggerate small changes); Holdings shows net worth at month end with
  no bar (a level, not an amount to compare by height); Check-in shows
  earned − spent, signed and with no bar (it crosses zero). The selected month
  gets the Selected Fill, its edge and a Field Teal bar. The strip is one
  Tab stop (roving tabindex): arrow keys, Home and End move and select. The
  month is App state, so every screen follows the same one (Check-in
  picks the last finished month when none is chosen yet); a month Holdings
  has no snapshots for stays selected and the screen says so.

### Change equation (Holdings)
- The Holdings focal moment, open on the page like Flow's headline pair:
  "Net worth change = Markets + Added" as three figures (34px for the
  change, 24px for the parts, tabular) joined by Text Dim operators (each drawn with the
  term after it, so a narrow screen wraps "= Markets" and "+ Added" whole), the
  change's percent beside it in Text Dim. Under it, the start and end in
  words, the definitions behind a closed `<details>` ("What Markets and
  Added mean · show", dotted Text Dim underline, the state spelled out
  rather than a caret), and a note when Markets can't
  cover every investment account (it names them and says their whole
  change is in Added). Below 480px the change takes its own line.

### Breakdown sum (Holdings)
- Stat tiles joined by their signs: Cash + Invested − Credit owed − Loans
  = Net worth, owed amounts shown as positive magnitudes after a minus
  (Text Dim operators: the sign carries the meaning, so it needs AA),
  never in Out Red. The Net worth tile carries the Selected edge. Below
  480px the tiles become a column sum, key left and value right.

### Account rows (Holdings)
- One card, one column header (Value / Change / Gain) and a fixed grid,
  so every group (Cash, Investments, Credit, Loans) lines up; the group
  total sits in the Value column. A stale sync is an Attention Amber note
  beside the account. Investment rows open their positions with a real
  button ("Show 4 positions" / "Hide positions", `aria-expanded`), never a
  caret ("Show 3 positions and cash" when it holds uninvested cash).
  Position rows never get a total: they don't sum to the account gain, but
  their values do sum to the account's value (uninvested cash is its own
  "Cash · cash" row). Position tables use fixed figure columns so every
  account's positions line up. A sold position says "sold" under Shares.
- **Owed rows** speak in their own frame: Change reads "up $763.14" /
  "down $410.00" (what's owed), not the net-worth sign beside an "owed"
  value, which reads backwards.
- **Reasons are visible, never `title`-only.** A missing figure is "—"
  with an sr-only reason, and the table carries one Text Dim footnote
  ("— under Unrealized: the broker reports no cost basis"). "from first
  snapshot" shows only on accounts that started later than the rest; when
  all did, the hero says it once.
- **Range switches keep the old figures** dimmed (`hd-busy`, 0.55 opacity
  after 150ms, `aria-busy`) until the new ones land; only the newest
  request may write. "Loading…" (`role=status`) appears only on first load.
- Below 720px figures move under the name with their own keys, and
  each position becomes two lines (holding and value, then labelled
  shares, gain and unrealized, the basis under the unrealized gain).

### Holdings charts
- Net worth (Field Teal) and Invested vs contributed (Kind-invest solid,
  Text Dim dashed for the contributed baseline, with a legend). Linear
  lines, no smoothing, one y-axis each, `role=img` with a sentence summary
  (dates, start, end, change, and the same caveat as the visible note),
  tooltip dates in the axis's month-day order. Cash is a margin note under
  the net worth chart, not a third line.

### Headline pair (Flow)
- The screen's focal moment, open on the page rather than in a card, so
  the lanes below read as subordinate. Two figures (34px, 650, tabular)
  split by a Hairline, with their keys in Text Dim above. Under them, the
  answer in words at 15px in Text colour, with the money in semibold.

### Lane card (Flow)
- A Card (Panel, Hairline, 12px) with a Row-name title, a 20px Figure, a
  one-line Text Dim note, then label/value rows split by Row Dividers.
  The three lanes are equal width and stack In → Out → Moved below 720px.
  A row with nothing behind it reads quiet (Text Dim) and isn't a button.

### Drill-down figure (Flow)
- Any figure with rows behind it is a button that filters the table to
  exactly those rows. At rest the amount carries a dotted Text Faint
  underline (the "you can act on this" mark, like the dashed category
  control); hover adds Row Hover and a Hairline inset, and the selected one
  gets the Selected Fill, a 2px Field Teal inset ring and `aria-pressed`.
  Rows bleed into the card padding so the whole line is the target; 44px
  tall on touch widths.

### Filter chip (Flow)
- A 999px capsule in Selected Fill with its edge: the drill-down path in
  600 ("Moved › Card payment"), the month in Text Dim, and a Clear button.
  Clearing (or Escape anywhere on the screen) returns focus to the figure
  that was selected.

### Transaction table (Flow)
- Sortable headers are buttons with `aria-sort`; amounts are signed and
  tabular, In amounts also In Green. Moved rows carry their group in place of a
  category, as a select dressed as the tag (CSS triangle caret, 24px tall):
  picking a group teaches the merchant, with one Undo for every row it
  changed; a card payment's tag stays plain (its match decides it). The
  name and amount stay full Text, never green.
  Category is a dashed-underline control that
  opens a select: a pointer pick saves, a keyboard pick saves on Enter or
  on leaving the field, Escape cancels. The row action sits at the end of
  the merchant cell on desktop (no column of its own to sit empty) and
  waits for hover or focus; on a phone it's always shown on the row's last
  line.
- **Side-grouped:** a card payment drills into both legs, each under a
  group header row ("From checking" / "Into cards") with its row count and
  subtotal, so the reader can see why Moved counts it once.
- **Two-line rows (below 720px):** each row becomes merchant and amount on
  line 1, then date, category, account chip and action on line 2,
  wrapping rather than truncating. The header row stays as the sort
  controls. Nothing is hidden at narrow widths or 200% zoom.

### Settings sections
- Settings opens with one upkeep line from core (`GET /api/upkeep`, the
  same numbers as the nav badge): what needs a look in Attention Amber,
  the rest in Text Dim, "Nothing needs you." when the total is zero.
- Each block is a foldable section: the title is a button with a chevron
  and `aria-expanded`; a one-line status sits beside it; the section's
  actions stay outside the button and work while it's closed. Sections
  that need nothing start closed.
- Row-level actions (nickname, hide, unlink) are small outline buttons:
  28px tall, Text Faint edge, Text Dim label. Never Note Blue.
- Anything that can't be undone confirms on the page, never with a
  browser dialog. Unlink asks once inline. Clearing data is the Danger
  zone: a red-edged panel listing what's lost (counts from core), a
  checkbox naming what can't be fetched again, then the red button.

## Do's and Don'ts

### Do:
- **Do** use the `--lg-*` custom properties in `app/src/ledger.css` for
  every new colour; they are the live token set.
- **Do** keep money figures tabular and right-aligned.
- **Do** give every control a visible border and a pressed and focused
  state, so it reads as pressable on the dark page. Form fields and small
  outline buttons use Text Faint for that edge: Hairline is about 1.3:1,
  and a control's boundary needs 3:1 (WCAG 1.4.11).
- **Do** put focus somewhere sensible after an action removes the control
  that had it, and keep an Undo toast up until it's used or dismissed.
- **Do** put one Key button per action group, at most.
- **Do** read token values with `getComputedStyle` when SVG presentation
  attributes can't take CSS variables (as `Holdings.tsx`'s charts do),
  never copy the hex.

### Don't:
- **Don't** write a hex value outside `:root`, or a font size in px:
  sizes are rem (13px = 0.8125rem) so text follows the browser's font-size
  setting. The px figures in this file are at the 16px default.
- **Don't** add drop shadows to content surfaces.
- **Don't** use colour for decoration; see the Meaning-Only Rule.
- **Don't** signal in versus out by colour alone.
- **Don't** put destructive or upkeep actions on an analysis screen; they
  belong in Settings.
