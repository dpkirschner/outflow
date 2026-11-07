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
  text-faint: "#5a6675"
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
  violet, investments are gold. Kind-invest also draws the cash series on
  the balances chart.

### Neutral
- **Ink Background** (ink-bg): the page.
- **Panel** (panel): cards, buttons, the lifted surface.
- **Recessed Panel** (panel-recessed): stat tiles and segmented-control
  tracks, a step *below* the panel.
- **Hairline** (hairline): every border and divider between surfaces.
- **Row Divider / Row Hover / Selected Fill** (row-divider, row-hover,
  selected-fill): list row separators, hover wash, and the selected segment
  or active nav tab. In CSS these are `--lg-divider`, `--lg-hover`,
  `--lg-selected` and its edge `--lg-selected-edge` (Flow uses them; older
  screens still repeat the hex values).
- **Text / Text Dim / Text Faint** (text, text-dim, text-faint): primary
  copy, secondary copy and labels, and metadata.

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
- **Label** (600, 10.5–11px, uppercase, 0.06–0.1em tracking): card headers
  and stat-tile keys.

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
which sits on the Ink Background. Hairlines separate everything. The one
place a surface floats is the slide-over, which dims the page with a scrim;
it still uses no drop shadow. The selected row is marked with an inset 3px
Field Teal bar on its leading edge rather than a lift.

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
  colour. Used for the time window (3mo / 6mo / 12mo / All) and sort modes.

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
- Small (10.5px, 600) text in a 4px-radius outline chip, tinted by account
  kind: ink text, a dark edge of the same hue and a near-black wash. The
  last four digits stay in the label.

### Navigation
- Text tabs at 12px, pushed to the right of the wordmark. At rest they're
  Text Faint; hover goes to Text Dim; the current tab gets the Selected
  Fill and Text colour. A count that needs attention sits in an Attention
  Amber capsule beside the label.

### Slide-over
- The detail view for a merchant or stream. It slides in from the right
  over a dark scrim, holds a bar rhythm chart, occurrence list and actions,
  and closes back to the list without losing scroll or fold state.

### Month strip (Flow)
- One button per month: a 20px bar (Out, scaled to the largest month),
  the short month and the short Out figure. The selected month gets the
  Selected Fill, its edge and a Field Teal bar. The strip is one Tab stop
  (roving tabindex): arrow keys, Home and End move and select.

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
  tabular, In amounts also In Green. Moved rows carry a "Moved · group" tag
  and dim the name and amount. Category is a dashed-underline control that
  opens a select: a pointer pick saves, a keyboard pick saves on Enter or
  on leaving the field, Escape cancels. The row action waits for hover or
  focus on desktop.
- **Side-grouped:** a card payment drills into both legs, each under a
  group header row ("From checking" / "Into cards") with its row count and
  subtotal, so the reader can see why Moved counts it once.
- **Two-line rows (below 720px):** each row becomes merchant and amount on
  line 1, then date, category, account chip and action on line 2,
  wrapping rather than truncating. The header row stays as the sort
  controls. Nothing is hidden at narrow widths or 200% zoom.

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
- **Do** mirror token values into constants when SVG presentation
  attributes can't read CSS variables (as `Balances.tsx` does), and keep
  them in sync.

### Don't:
- **Don't** use the legacy `theme.css` palette (`--bg`, `--card`,
  `--accent`, the teal-to-blue gradient) for new work. Only the auth gate,
  toast and a few fallbacks still read it.
- **Don't** add drop shadows to content surfaces.
- **Don't** use colour for decoration; see the Meaning-Only Rule.
- **Don't** signal in versus out by colour alone.
- **Don't** put destructive or upkeep actions on an analysis screen; they
  belong in Settings.
