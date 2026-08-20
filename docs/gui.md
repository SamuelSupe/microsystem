# GUI profile

The GUI profile is enabled when the VirtIO-GPU, keyboard and tablet devices are
present. It starts all `SERVICE_COUNT=12` resident services with ASIDs
`0x20..0x2b`; the serial profile keeps the eight-service set (`8/8`). The image
contains 24 bootfs entries (23 static ELFs plus `etc/services`). The desktop is
fixed at 1024×768 XRGB8888 and is presented through windowd's EL0 VirtIO-GPU
path.

## Device and capability boundary

Windowd owns the scanout and its GUI memory pool. `devmgr` owns the PCI/BAR,
queue, DMA and IRQ setup; applications do not receive any of those
capabilities. Mica GUI sessions receive only a 64 KiB command `FrameRegion`, a
4 KiB event `Frame`, a script notification carrying `script::EVENT_GUI`, and a
dedicated GUI endpoint. The endpoint is registered by init and bound to the
session PID/token; a message cannot forge its client identity. The framebuffer,
GPU BAR, input queues, DMA, IRQ and raw device syscalls remain inaccessible.

There are three built-in clients (Terminal, Files and Monitor) and up to eight
dynamic Mica clients, for a desktop capacity of 11 windows. Closing a dynamic
client unregisters its endpoint and reclaims its command/event resources; the
slot can be reused.

### Desktop launchers

The desktop has a fixed five-entry icon registry owned by init: Terminal (id 1),
Files (id 2), Monitor (id 3), Reader (id 4) and Editor (id 5). The first three
icons are at the lower-left;
Reader is at `(864,650)` and Editor at `(940,650)`, each 64×64. The drawing
`Icon` command above is only a validated display-list primitive; it does not
register or launch applications. Clicking a built-in icon focuses/restores its
existing window. Reader and Editor clicks send only the enum application id
over the init-owned `GUI_LAUNCH_ENDPOINT` (capability slot 84); init selects the
fixed script path, manifest and entry policy for that id. Windowd deduplicates
active and pending launches, so a repeated click does not create another
session. Init binds a dynamic registration's application id in registration
word 3, and rejects unknown/non-launchable ids. The effective Mica permissions
remain the manifest ∩ launcher `--allow` ∩ entry-policy intersection.

The fixed mappings are deliberately narrow: Reader runs the bundled browser
example with `gui.window` and unscoped `net.browse`; Editor runs the
bundled editor with `gui.window`, `fs.read:/data` and `fs.write:/data`, passing
`/data/note.txt`. The window server cannot supply an arbitrary script path or
policy through this endpoint. Closing a launcher-created window follows the
same `CloseRequested` and endpoint-reclaim lifecycle as any dynamic Mica
client; a subsequent click can reuse the slot.

### Visual presentation

The 1024×768 desktop is code-drawn in windowd with a layered navy background,
fine vertical separators and a darker taskbar. Windows use an offset drop
shadow, a bordered 28-pixel title bar and separate active/inactive chrome; the
active title color is `#1e3b63` with a brighter border/accent, while inactive
titles use muted text. The title controls are explicit 12×12 `X`, `-` and `+`
buttons, and the bottom-right resize grip is rendered as three small corner
marks. Focus is also reflected by the taskbar button fill and its bottom accent
strip, so a minimized window remains discoverable without changing the window
protocol or geometry contract.

The five launcher cards use distinct code-drawn symbols (terminal prompt,
folder, meter bars, reader pages and editor sheet) rather than a shared generic
glyph. The built-in Terminal, Files and Monitor clients draw bounded content
panels: a dark console with an accent rail, a places/sidebar and file rows, and
CPU/memory/storage meters. Mica's retained widgets use the same navy panel/input
palette; focused buttons, text inputs, checkboxes and lists receive a two-pixel
accent outline, while selected list rows use the dark accent fill. These are
renderer-level visuals only; the GUI ABI, launch endpoint, capability boundary
and validated display-list commands remain unchanged.

The Terminal command surface is `ls/list [path]`, `cat/read <path>`,
`stat <path>`, `touch/create <path>`, `cp <source> <destination>`,
`write <path> <text>`, `mkdir <path>`, `rmdir <path>`,
`mv/rename <source> <destination>`, `rm/remove/unlink <path>`, `fsync <path>`
and `sync`. Paths are absolute and there is no working-directory
state. Terminal sends filesystem requests through its dedicated
`TERMINAL_FILESYSTEM_ENDPOINT` and the `GUI_TERMINAL_COMMANDS` 4 KiB shared
frame; the MFS broker maps that frame at a terminal-only path boundary, separate
from block DMA and other filesystem payload frames. Each command is limited to
512 bytes and replies/output to 4 KiB, with the console viewport following the
newest lines. The serial shell and GUI terminal share the same filesystem
parser and aliases.

The materialized-paths review also tightened redraw and event behavior without
changing the GUI wire contract: windowd dirty-region culling skips
non-intersecting windows, display lists, icons and taskbar work, while Files
and Monitor block on event IPC (`wait=event-blocked`) instead of spinning on
`yield`. OrbStack QEMU TCG remains functional/regression evidence for these
bounded changes, not a native-hardware performance benchmark.

The GUI-performance renderer additionally merges overlapping damage, redraws
only local `DesktopVisual` windows for drag/resize/focus/Terminal updates, and
culls clean display-list command/text/glyph work. Its Unifont cache is 2-way
set-associative and input delivery is batched at 16 events; these changes do
not alter the ABI, permission or endpoint protocol.

## GUI wire contract

GUI is `protocol::GUI = 6`, version 1. `ThreadLaunchV1` remains accepted for
non-GUI Mica sessions; GUI uses compatible `ThreadLaunchV2` with three appended
capability slots. The GUI command frame contains a `PresentHeaderV1`, at most
4,096 validated commands, 48 KiB of text and 16 damage rectangles. Windowd
validates the complete UTF-8/title/payload/hash/coordinate/clip/command stream
before atomically replacing the client's display list. Invalid Present requests
return `Invalid` and leave the previous display list intact. Refresh is merged
and capped at 60 Hz; validated damage rectangles bound the redraw region.

The six drawing commands are `Clear`, `FillRect`, `StrokeRect`, `Text`, `Icon`
and `SetClip`. Events in the SPSC ring are `PointerMove`, `PointerButton`,
`PointerWheel`, `Key`, `TextInput`, `Focus`, `Configure`, `Expose` and
`CloseRequested`. Pointer moves may be coalesced when the ring is full; button,
key, configure, expose and close events are retained with bounded backpressure.
The window server supplies title bars, borders, focus, z-order, drag/resize,
minimize, maximize, restore, taskbar and Alt+Tab. Client coordinates are always
relative to the client area and clipped by windowd. Partial Present is bounded
across user-rt, kernel, GPU and windowd; pointer movement contributes bounded
 damage, and pending input is batched at 16. The acceptance marker is
`[gui] EL0 windowd partial-present=true rect=1024x720+0,48`; the renderer
marker is `damage-merge=true local-window-damage=true command-cull=true
glyph-cache=2way input-batch=16`; the net marker reports `rx-buffers=2`.

Windowd loads `/.system/fonts/unifont-17.0.05.ufb` into a bounded 128-entry,
2-way set-associative glyph cache. If the runtime font cannot be loaded it falls back to the built-in ASCII
font and replacement glyphs. This cache bound changes no GUI wire, capability,
security or resource limit.

## Mica GUI API

`mica --gui --timeout 86400s` requires a file path; GUI `-e` and GUI REPL are rejected. The file
must declare `--!allow gui.window`. Effective permission is the intersection of
the script manifest, launcher `--allow` rules and the entry-point policy (the
SSH maximum is `/.system/ssh/mica-policy`). Without a GUI device/windowd,
launch returns `NotSupported`. The timeout parser accepts `1ms` through `24h`;
non-GUI Mica is hard-capped at 60 seconds, while GUI file sessions may use the
full 24-hour budget.

```lua
--!mica 1
--!allow gui.window
local gui = require("gui")

local label = gui.label { text = "Count: 0" }
local app = assert(gui.window { title = "Counter", width = 420, height = 260 })
app:set_root(gui.column {
    padding = 16,
    gap = 12,
    children = {
        label,
        gui.button {
            text = "Increment",
            on_click = function()
                label:set_text("clicked")
                app:invalidate()
            end,
        },
        gui.text_input {
            placeholder = "Type here",
            on_submit = function(text) label:set_text(text) end,
        },
    },
})
app:run()
```

The module provides `window`, `label`, `button`, `text_input`, `checkbox`,
`list`, `scroll`, `row`, `column`, `spacer` and `canvas`. Window methods are
`set_root`, `set_title`, `invalidate`, `present`, `run` and `close`; `present()`
synchronously submits a dirty retained frame before returning. Widgets expose
`set_text`, `set_checked` and `set_items`. Interactive widgets support
`on_click`, `on_change`, `on_submit` and `on_select` callbacks. Row/column/scroll
use bounded integer flex layout (`padding`, `gap`, fixed dimensions and
`grow`). A `Scroll` container handles bounded wheel scrolling but is not itself
a focus/click target. `List` and `Canvas` own their own client clips; `Canvas`
commands remain limited to the same six validated drawing commands.

Callbacks can also be attached after construction with
`widget:set_callback("click"|"change"|"submit"|"select", function)`, or
cleared by passing `nil`. The callback values remain in the GUI host's GC roots
while the event loop yields.

Callbacks are ordinary Mica closures: captured state remains available across
successive GUI events, which is used by the Reader's navigation/history
controller.

One Mica GUI script owns one top-level window and the retained widget tree. A
close request first reaches the script as `CloseRequested`; init allows up to
two seconds for cleanup before terminating the session and reclaiming all
resources. The GUI host keeps callback values in its GC roots while yielding.

The shipped `/.system/examples/mica/browser.mica` demonstrates a complete
single-window application assembled from these widgets. It uses a row of
buttons and a text input for navigation, a list for paged document lines, and
callback registration through `widget:set_callback(name, function)`. The
browser remains subject to the same one-window, bounded-widget and exact
`gui.window` permission contract; it does not add a browser-specific drawing
or input capability. Its fixed entry policy adds only `net.browse`: the Reader
uses `http.get` for HTTP/HTTPS GET to the dynamic address-bar host. Raw
resolve/TCP/UDP and POST/PUT/PATCH/DELETE remain exact `net.connect` paths;
page links are same-origin, while an explicit address-bar URL may cross origin.
TLS roots/time/hostname validation and the existing body/content limits are
unchanged.

The shipped `/.system/examples/mica/editor.mica` demonstrates a bounded text
editor using a path `text_input`, document `list`, selected-line `text_input`,
buttons and a status `label`. It is launched with `gui.window`,
`fs.read:/data` and `fs.write:/data`; all three permissions remain subject to
manifest/launcher/entry-policy intersection. The editor accepts at most 32
lines and 32,512 bytes, normalizes CRLF/CR, and saves through atomic WriteFile
plus fsync without adding any GUI capability.

## Acceptance evidence

The final OrbStack run passed `make build`, `make test` and `make fsck` (all exit
0). Host suites were MFS 16/16, capability ABI 9/9,
GUI command-stream 6/6, Desktop 9/9,
kernel 11/11 and Mica 24/24 (**75/75 total**). The dynamic Counter gate reached these lifecycle
markers in order:

```text
[gui] mica registration requested endpoint=0
[gui] mica registration received endpoint=0
[gui] mica client registered command=65536 event=4096 endpoint-isolated=true
[mica] gui presented widgets=true atomic=true isolated=true
[mica] vm returned=true
[mica] exiting=true
[gui] mica client unregistered endpoint=0 resources-reclaimed=true
```

SSH status was 0; stdout was empty and stderr contained only the expected
known-host warning. QMP clicked the button, entered ASCII and Unicode text,
resized/maximized/restored/minimized the window, used the taskbar, and closed
the window. All decoded screenshots were 1024×768 and produced CRCs:

```text
desktop  b1998cf3
mica-pre 7d6a4f44
button   e8128290
ascii    9f1f4603
unicode  60f7c6f6
resize   f19b1e8d
restore  957bdb75
max      524aacc7
min      083b1435
taskbar  c9110363
second-pre    cca2e7d6
second-button 40858a7f
second-text   a445076b
after-close-reuse 35366f14
stable-client 9014e4dd
```

Unicode codepoint accumulators delivered/rendered were `4`, `78`, `1250` and
`20013`. The refreshed visual gate used active title color `#1e3b63`; geometry
checks reported max `962`, active-title row `80` resize/restore `418`,
minimized active-title row `90` equal to `0`, taskbar active row `750` equal to
`121`, partial Present `1024x720+0,48`, and stable client crop `9014e4dd`.
Endpoint 1 registered, presented and
reclaimed twice and was reused; endpoint 0 was reclaimed. The second and third
sessions were launched through the persistent serial shell while the first
remained on SSH (sshd serves one session), proving GUI-session concurrency
rather than SSH transport concurrency. No panic or DMA fault occurred and
cleanup left no QEMU/container residue. The final MFS1 check was:

```text
MFS1 clean generation=2480 transactions=2323 entries=24 used_blocks=13431
```

The unified FP/SIMD acceptance logs are `target/unified-fp-simd-final.log` and
`target/unified-fp-simd-fsck.log`; GUI/QMP details remain in `target/gui-qemu.log`
and `target/gui-qemu.qmp.jsonl`.

The GUI browser fixture served exactly one `GET /index.html` and one
`GET /next.html`, rejected the cross-origin `evil` request, and observed two
HTTP 200 Reader loads. The separate Mica network fixture recorded
`GET /index.txt?browse=1` after fragment removal and DNS for `mica.test`; the
Mica combo marker records raw resolve/TCP/UDP and POST denials plus trusted TLS,
the presented root-chain, P-384 `CertificateVerify`, long CA DNS name and all
four TLS rejection cases. Endpoint 1 registered, presented, reclaimed,
and the script exited with status 0. Browser screenshot CRCs were `2286d988`
(pre), `5dab179e` (same-origin link), and `f60453d9` (cross-origin rejection).
The browser evidence is in `target/gui-browser-fixture.log`,
`target/mica-dns-fixture.log`, `target/mica-qemu.log`,
`target/unified-browser-public-internet-final.log` and
`target/unified-browser-public-internet-fsck.log`.
The public Reader image-bound capture
[`target/browser-internet-reader-imagebound-final.png`](../target/browser-internet-reader-imagebound-final.png)
shows `https://example.com/` with `HTTP 200 — https://example.com` and
`# Example Domain`.

The unified Terminal filesystem sequence was:

```text
[gui] terminal filesystem command=mkdir status=ok
[gui] terminal filesystem command=write status=ok
[gui] terminal filesystem command=stat status=ok
[gui] terminal filesystem command=mv status=ok
[gui] terminal filesystem command=cat status=ok
[gui] terminal filesystem command=ls status=ok
[gui] terminal filesystem command=rm status=ok
[gui] terminal filesystem command=rm status=ok
[gui] terminal filesystem command=sync status=ok
```

The Terminal help screenshot decoded to 1024×768 with 140 foreground rows and
CRC `b3c61c67`, proving the 4 KiB shared-frame output path rather than the old
40-byte inline reply. These markers and the final fsck tuple are in
`target/unified-browser-public-internet-final.log` and
`target/unified-browser-public-internet-fsck.log`.

The Editor gate then reused the dynamic slot and passed pointer sequence
`4, 5, 6, 10, 9`, two loads, one atomic/fsync save, reload, CloseRequested
and endpoint reclaim with `mica: pid=14 status=0`. Its decoded full-frame
CRCs for pre/focused/typed/apply/saved/reload were
`3eeb31ac/5ea9fc60/dcccc6a3/da105c73/18f38838/7e895e1d`; the first-row
persistence crop was `0a8ab59f` for both Apply and Reload.

The dedicated desktop Editor-icon gate also exited 0. It observed application
id 5 click/accept, endpoint request/receive/register and an atomic Present;
duplicate click and minimize/restore did not create another session. Close
reclaimed the endpoint, and a second click registered/presented/reclaimed a
fresh session. Unified icon JSON reported launches=2, registrations=2,
Presents=2 and reclaims=2. Screenshot CRCs were `d2a43609` (pre),
`f31681cc` (active), `ed8a4c52` (restored) and `f31681cc` (restart).
Filesystem read plus Present
proved the document load; detached-session stdout was not used as a marker.

## Deliberate limits

This release is a single-window retained toolkit, not a general desktop
framework. Dynamic GUI registration is supported, but each app still has one
top-level window. There are no multi-window apps, popup/modal menus,
tables/trees/tabs, drag-and-drop, clipboard, IME, complex text shaping,
arbitrary-font selection, image decoding, transparency, animation timelines,
3D/GPU acceleration, dynamic native modules or arbitrary native application
binaries.
