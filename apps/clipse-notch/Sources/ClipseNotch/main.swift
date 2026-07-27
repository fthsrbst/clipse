// ClipseNotch — the panel that lives under a MacBook's notch.
//
// # Why this is a sidecar and not part of the Tauri app
//
// A borderless NSPanel at `.statusBar` level, positioned against the notch, is
// not something a webview can be. It needs AppKit directly. Running it as a
// separate process launched by the app also means a crash here takes down a
// decoration, not the clipboard.
//
// # Why it speaks stdin/stdout and not the daemon's protocol
//
// The daemon's IPC is length-prefixed MessagePack over a unix socket.
// Reimplementing that in Swift would mean a second copy of the wire format
// that has to be kept in step with `clipse-ipc` forever. Instead the Tauri
// app — which already holds a connection — pushes newline-delimited JSON in,
// and reads actions back out. One protocol, one place it can drift.
//
// # Status
//
// **This file has never been compiled or run.** It was written on a Windows
// machine with no Swift toolchain and no Mac. CI builds it on a macOS runner,
// so "does it compile" has an answer; "does the panel sit correctly under the
// notch" does not, and cannot until someone looks at it. See
// `docs/manual-verification.md` §F3.

import AppKit
import Foundation

// MARK: - Wire format

/// One clip, as the Tauri app sends it. A deliberately narrow view of
/// `clipse_core::Clip` — the notch shows three lines, so it is given three
/// lines' worth of information and nothing else.
struct NotchClip: Decodable {
    let id: String
    let preview: String
    let kind: String
    let sourceLabel: String
    /// True when the clip arrived from another device, which is what the
    /// arrival animation is for.
    let fromPeer: Bool
}

enum Incoming: Decodable {
    case clips([NotchClip])
    case hide

    private enum CodingKeys: String, CodingKey { case type, clips }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .type) {
        case "clips":
            self = .clips(try container.decode([NotchClip].self, forKey: .clips))
        case "hide":
            self = .hide
        case let other:
            throw DecodingError.dataCorruptedError(
                forKey: .type, in: container,
                debugDescription: "unknown message type \(other)")
        }
    }
}

/// What the panel asks the app to do. Written as one JSON object per line so
/// the reader on the other side needs no framing.
func emit(_ action: String, clipId: String) {
    let payload = ["action": action, "clipId": clipId]
    guard let data = try? JSONSerialization.data(withJSONObject: payload),
          let line = String(data: data, encoding: .utf8)
    else { return }
    print(line)
    fflush(stdout)
}

// MARK: - The panel

final class NotchPanel: NSPanel {
    init(contentRect: NSRect) {
        super.init(
            contentRect: contentRect,
            // Borderless and non-activating: the panel must never take focus
            // from whatever the user is typing in, or hovering it would
            // interrupt the very work they are pasting into.
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false)

        isFloatingPanel = true
        level = .statusBar
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        hidesOnDeactivate = false
        // Visible over full-screen apps, and it should not become a space of
        // its own when the user swipes between desktops.
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        ignoresMouseEvents = false
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

// MARK: - Placement

enum Placement {
    /// Where the panel sits on `screen`.
    ///
    /// `safeAreaInsets.top` is non-zero exactly on a display with a notch, and
    /// is the height the menu bar was pushed down to clear it. Using it rather
    /// than a hard-coded number means an external monitor — where the inset is
    /// zero — gets a panel hanging from the top of the screen instead of one
    /// floating mysteriously below the menu bar.
    static func frame(for screen: NSScreen, size: NSSize) -> NSRect {
        let visible = screen.frame
        let inset = screen.safeAreaInsets.top
        let top = visible.maxY - (inset > 0 ? inset : screen.frame.maxY - screen.visibleFrame.maxY)
        return NSRect(
            x: visible.midX - size.width / 2,
            y: top - size.height,
            width: size.width,
            height: size.height)
    }

    static var current: NSScreen? {
        // The screen with the notch is the built-in one; when there is none,
        // whichever screen the pointer is on is the one the user is looking at.
        NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 })
            ?? NSScreen.screens.first(where: { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) })
            ?? NSScreen.main
    }
}

// MARK: - Content

final class ClipRow: NSView {
    let clip: NotchClip
    private let onActivate: (NotchClip) -> Void

    init(clip: NotchClip, onActivate: @escaping (NotchClip) -> Void) {
        self.clip = clip
        self.onActivate = onActivate
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 8

        let label = NSTextField(labelWithString: clip.preview)
        label.lineBreakMode = .byTruncatingTail
        label.maximumNumberOfLines = 1
        label.font = .systemFont(ofSize: 12)
        label.textColor = .labelColor
        label.translatesAutoresizingMaskIntoConstraints = false

        let source = NSTextField(labelWithString: clip.sourceLabel)
        source.font = .systemFont(ofSize: 10)
        source.textColor = .secondaryLabelColor
        source.translatesAutoresizingMaskIntoConstraints = false

        addSubview(label)
        addSubview(source)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            source.leadingAnchor.constraint(greaterThanOrEqualTo: label.trailingAnchor, constant: 8),
            source.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            source.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: 28),
        ])

        registerForDraggedTypes([.string])

        if clip.fromPeer { animateArrival() }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not loaded from a nib") }

    /// A clip that came from another device slides in and settles. The point
    /// is not decoration: it is the only moment the user is told *where* the
    /// thing on their clipboard came from.
    private func animateArrival() {
        alphaValue = 0
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.28
            context.timingFunction = CAMediaTimingFunction(controlPoints: 0.22, 1, 0.36, 1)
            animator().alphaValue = 1
        }
    }

    override func mouseDown(with event: NSEvent) {
        onActivate(clip)
    }

    // Dropping text on the panel is a way to put something on the clipboard
    // without leaving the app you are in.
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { .copy }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard let text = sender.draggingPasteboard.string(forType: .string) else { return false }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        return true
    }
}

// MARK: - Controller

final class NotchController: NSObject, NSApplicationDelegate {
    private var panel: NotchPanel?
    private var clips: [NotchClip] = []
    private var expanded = false
    private var trackingArea: NSTrackingArea?

    private static let collapsedSize = NSSize(width: 180, height: 8)
    private static let expandedSize = NSSize(width: 360, height: 104)

    func applicationDidFinishLaunching(_ notification: Notification) {
        // No Dock icon and no menu bar: this is a decoration on someone else's
        // window, not an app they switch to.
        NSApp.setActivationPolicy(.accessory)
        buildPanel()
        readStdin()
    }

    private func buildPanel() {
        guard let screen = Placement.current else { return }
        let frame = Placement.frame(for: screen, size: Self.collapsedSize)
        let panel = NotchPanel(contentRect: frame)

        let content = NSVisualEffectView(frame: NSRect(origin: .zero, size: frame.size))
        content.material = .hudWindow
        content.blendingMode = .behindWindow
        content.state = .active
        content.wantsLayer = true
        content.layer?.cornerRadius = 10
        content.autoresizingMask = [.width, .height]
        panel.contentView = content

        panel.orderFrontRegardless()
        self.panel = panel
        installTracking()
    }

    private func installTracking() {
        guard let content = panel?.contentView else { return }
        if let existing = trackingArea { content.removeTrackingArea(existing) }
        let area = NSTrackingArea(
            rect: content.bounds,
            options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
            owner: self)
        content.addTrackingArea(area)
        trackingArea = area
    }

    override func mouseEntered(with event: NSEvent) { setExpanded(true) }
    override func mouseExited(with event: NSEvent) { setExpanded(false) }

    private func setExpanded(_ value: Bool) {
        guard value != expanded, let panel, let screen = Placement.current else { return }
        expanded = value

        let size = value ? Self.expandedSize : Self.collapsedSize
        let frame = Placement.frame(for: screen, size: size)

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.22
            context.timingFunction = CAMediaTimingFunction(controlPoints: 0.22, 1, 0.36, 1)
            panel.animator().setFrame(frame, display: true)
        }
        render()
    }

    private func render() {
        guard let content = panel?.contentView else { return }
        content.subviews.forEach { $0.removeFromSuperview() }
        guard expanded else { return }

        var y = content.bounds.height - 34
        // Three, because that is what fits under a notch without becoming a
        // window. The full history is one hotkey away.
        for clip in clips.prefix(3) {
            let row = ClipRow(clip: clip) { [weak self] clip in
                self?.activate(clip)
            }
            row.frame = NSRect(x: 6, y: y, width: content.bounds.width - 12, height: 28)
            content.addSubview(row)
            y -= 32
        }
        installTracking()
    }

    private func activate(_ clip: NotchClip) {
        emit("paste", clipId: clip.id)
        setExpanded(false)
    }

    /// One JSON object per line, on a background queue so the run loop keeps
    /// drawing while the app is quiet.
    private func readStdin() {
        DispatchQueue.global(qos: .utility).async {
            while let line = readLine(strippingNewline: true) {
                guard let data = line.data(using: .utf8),
                      let message = try? JSONDecoder().decode(Incoming.self, from: data)
                else { continue }

                DispatchQueue.main.async {
                    switch message {
                    case .clips(let clips):
                        self.clips = clips
                        self.render()
                    case .hide:
                        self.setExpanded(false)
                    }
                }
            }
            // stdin closed: the app that launched us is gone, and a panel with
            // nothing behind it is worse than no panel.
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }
}

let app = NSApplication.shared
let controller = NotchController()
app.delegate = controller
app.run()
