// Poof.swift — the little puff of smoke when a favourite is dragged out of
// the sidebar. `NSAnimationEffect.disappearingItemDefault` is deprecated
// (macOS 14), so this draws its own: a few soft puffs that swell and fade in
// a click-through window at the drop point. Under Reduce Motion the puffs
// only fade.

import AppKit

@MainActor
enum Poof {
    private static var windows: [NSWindow] = []

    /// Plays the poof centred on a screen point.
    static func show(at screenPoint: NSPoint) {
        let size: CGFloat = 64
        let w = NSPanel(contentRect: NSRect(x: screenPoint.x - size / 2, y: screenPoint.y - size / 2, width: size, height: size),
                        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        w.isOpaque = false
        w.backgroundColor = .clear
        w.hasShadow = false
        w.ignoresMouseEvents = true
        w.level = .popUpMenu
        w.isReleasedWhenClosed = false
        w.setAccessibilityElement(false)
        let v = NSView(frame: NSRect(x: 0, y: 0, width: size, height: size))
        v.wantsLayer = true
        w.contentView = v
        let reduce = Motion.reduce
        let colour = NSColor.secondaryLabelColor.withAlphaComponent(0.55).cgColor
        // Five puffs around the centre, like a small cloud.
        let puffs: [(CGFloat, CGFloat, CGFloat)] = [(0, 0, 14), (-11, 5, 10), (11, 5, 10), (-7, -8, 9), (8, -8, 9)]
        for (i, (dx, dy, r)) in puffs.enumerated() {
            let l = CALayer()
            l.bounds = CGRect(x: 0, y: 0, width: r * 2, height: r * 2)
            l.cornerRadius = r
            l.backgroundColor = colour
            l.position = CGPoint(x: size / 2 + dx, y: size / 2 + dy)
            l.opacity = 0
            v.layer?.addSublayer(l)
            let fade = CAKeyframeAnimation(keyPath: "opacity")
            fade.values = [0, 1, 0]
            fade.keyTimes = [0, 0.25, 1]
            let group = CAAnimationGroup()
            group.duration = reduce ? 0.25 : 0.42
            group.beginTime = CACurrentMediaTime() + (reduce ? 0 : Double(i) * 0.02)
            group.fillMode = .both
            group.isRemovedOnCompletion = false
            if reduce {
                group.animations = [fade]
            } else {
                let grow = CABasicAnimation(keyPath: "transform.scale")
                grow.fromValue = 0.4
                grow.toValue = 1.5
                grow.timingFunction = CAMediaTimingFunction(name: .easeOut)
                let drift = CABasicAnimation(keyPath: "position")
                drift.toValue = CGPoint(x: size / 2 + dx * 1.6, y: size / 2 + dy * 1.6 + 4)
                group.animations = [fade, grow, drift]
            }
            l.add(group, forKey: "poof")
        }
        w.orderFrontRegardless()
        windows.append(w)
        Timer.scheduledTimer(withTimeInterval: 0.6, repeats: false) { _ in
            MainActor.assumeIsolated {
                w.orderOut(nil)
                windows.removeAll { $0 === w }
            }
        }
    }
}
