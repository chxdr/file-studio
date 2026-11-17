// IconGridView.swift — the icon view (M3): a TileGrid of thumbnails with
// names, and a thin bar with the thumbnail-size slider (the core keeps the
// size in `grid.icon_size`; ⇧⌘= and ⇧⌘- step it).

import AppKit

@MainActor
final class IconGridView: NSView {
    private unowned let model: AppModel
    let grid: TileGrid
    private let bar = NSView()
    private let slider = NSSlider()
    private let small = NSImageView()
    private let big = NSImageView()
    private var observer: ObserverToken?
    private var sendTimer: Timer?
    private var pendingSize: Int?

    init(model: AppModel) {
        self.model = model
        grid = TileGrid(model: model, axis: .vertical)
        super.init(frame: .zero)
        wantsLayer = true
        grid.rename = RenameController(model: model, host: grid)
        let range = (model.engine as? GridEngineAPI)?.iconSizeRange() ?? 32...256
        slider.minValue = Double(range.lowerBound)
        slider.maxValue = Double(range.upperBound)
        slider.isContinuous = true
        slider.controlSize = .small
        slider.target = self
        slider.action = #selector(sliderMoved(_:))
        slider.toolTip = "Thumbnail size (⇧⌘= bigger, ⇧⌘- smaller)"
        slider.setAccessibilityLabel("Thumbnail size")
        small.image = NSImage(systemSymbolName: "photo", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 9, weight: .regular))
        big.image = NSImage(systemSymbolName: "photo", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 13, weight: .regular))
        for v in [small, big] { v.contentTintColor = .secondaryLabelColor }
        for v in [grid.view, bar] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        for v in [small, slider, big] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            bar.addSubview(v)
        }
        let line = NSBox()
        line.boxType = .separator
        line.translatesAutoresizingMaskIntoConstraints = false
        bar.addSubview(line)
        NSLayoutConstraint.activate([
            grid.view.topAnchor.constraint(equalTo: topAnchor),
            grid.view.leadingAnchor.constraint(equalTo: leadingAnchor),
            grid.view.trailingAnchor.constraint(equalTo: trailingAnchor),
            grid.view.bottomAnchor.constraint(equalTo: bar.topAnchor),
            bar.leadingAnchor.constraint(equalTo: leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: trailingAnchor),
            bar.bottomAnchor.constraint(equalTo: bottomAnchor),
            bar.heightAnchor.constraint(equalToConstant: 26),
            line.topAnchor.constraint(equalTo: bar.topAnchor),
            line.leadingAnchor.constraint(equalTo: bar.leadingAnchor),
            line.trailingAnchor.constraint(equalTo: bar.trailingAnchor),
            big.trailingAnchor.constraint(equalTo: bar.trailingAnchor, constant: -12),
            big.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
            slider.trailingAnchor.constraint(equalTo: big.leadingAnchor, constant: -6),
            slider.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
            slider.widthAnchor.constraint(equalToConstant: 110),
            small.trailingAnchor.constraint(equalTo: slider.leadingAnchor, constant: -6),
            small.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
        ])
        syncSlider()
        observer = model.observe { [weak self] e in
            if case .settingsChanged = e { self?.syncSlider() }
        }
    }
    required init?(coder: NSCoder) { fatalError() }

    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() { layer?.backgroundColor = NSColor.textBackgroundColor.cgColor }

    func show(_ t: TabModel) { grid.show(.tab(t.api), owner: t.id) }
    func detach() {
        grid.detach()
        sendTimer?.invalidate()
        sendTimer = nil
    }

    private func syncSlider() {
        guard pendingSize == nil else { return }
        slider.doubleValue = Double(GridSettings.iconSize(model))
    }

    /// The slider sends at most ~30 sizes a second to the core.
    @objc private func sliderMoved(_ s: NSSlider) {
        pendingSize = Int(s.doubleValue.rounded())
        guard sendTimer == nil else { return }
        send()
        sendTimer = Timer.scheduledTimer(withTimeInterval: 0.033, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                if self.pendingSize == nil { self.sendTimer?.invalidate(); self.sendTimer = nil } else { self.send() }
            }
        }
    }

    private func send() {
        guard let size = pendingSize else { return }
        pendingSize = nil
        (model.engine as? GridEngineAPI)?.setIconSize(size)
    }
}
