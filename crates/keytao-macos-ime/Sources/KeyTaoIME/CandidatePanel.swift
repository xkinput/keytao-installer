import Cocoa

/// A lightweight floating panel that displays Rime candidates.
/// Positioned just below the cursor of the active text field.
class CandidatePanel: NSPanel {

    var onSelect: ((Int) -> Void)?
    var onPageChange: ((Bool) -> Void)?

    private let stackView = NSStackView()
    private let prevButton = NSButton()
    private let nextButton = NSButton()

    // MARK: – Init

    override init(contentRect: NSRect, styleMask style: NSWindow.StyleMask,
                  backing backingStoreType: NSWindow.BackingStoreType, defer flag: Bool) {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 400, height: 36),
                   styleMask: [.nonactivatingPanel, .borderless],
                   backing: .buffered, defer: false)
        configure()
    }

    convenience init() {
        self.init(contentRect: .zero, styleMask: [], backing: .buffered, defer: false)
    }

    private func configure() {
        isFloatingPanel = true
        level = .popUpMenu
        isOpaque = false
        backgroundColor = NSColor(named: "CandidateBackground") ?? NSColor.windowBackgroundColor
        hasShadow = true
        isMovable = false
        hidesOnDeactivate = false

        let container = NSView()
        container.wantsLayer = true
        container.layer?.cornerRadius = 8
        container.layer?.masksToBounds = true

        stackView.orientation = .horizontal
        stackView.spacing = 2
        stackView.edgeInsets = NSEdgeInsets(top: 6, left: 8, bottom: 6, right: 8)
        stackView.translatesAutoresizingMaskIntoConstraints = false

        container.addSubview(stackView)
        NSLayoutConstraint.activate([
            stackView.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            stackView.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            stackView.topAnchor.constraint(equalTo: container.topAnchor),
            stackView.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])

        contentView = container
    }

    // MARK: – Update

    func update(texts: [String], comments: [String],
                page: Int, isLastPage: Bool, selectKeys: String,
                near cursorRect: NSRect) {

        // Rebuild candidate buttons
        stackView.arrangedSubviews.forEach { $0.removeFromSuperview() }

        let keys = Array(selectKeys.isEmpty ? "1234567890" : selectKeys)

        for (i, text) in texts.enumerated() {
            let btn = makeButton(
                label: "\(keys[safe: i] ?? Character("?"))\u{fe0e}.\(text)",
                comment: comments[safe: i] ?? ""
            )
            let idx = i
            btn.target = self
            btn.tag = idx
            btn.action = #selector(candidateClicked(_:))
            stackView.addArrangedSubview(btn)

            if i < texts.count - 1 {
                let sep = NSView()
                sep.wantsLayer = true
                sep.layer?.backgroundColor = NSColor.separatorColor.cgColor
                sep.widthAnchor.constraint(equalToConstant: 1).isActive = true
                stackView.addArrangedSubview(sep)
            }
        }

        // Page navigation
        if page > 0 || !isLastPage {
            let sep = NSView()
            sep.wantsLayer = true
            sep.layer?.backgroundColor = NSColor.separatorColor.cgColor
            sep.widthAnchor.constraint(equalToConstant: 1).isActive = true
            stackView.addArrangedSubview(sep)

            if page > 0 {
                let prev = makeNavButton(symbol: "chevron.left")
                prev.action = #selector(prevPage)
                prev.target = self
                stackView.addArrangedSubview(prev)
            }
            if !isLastPage {
                let next = makeNavButton(symbol: "chevron.right")
                next.action = #selector(nextPage)
                next.target = self
                stackView.addArrangedSubview(next)
            }
        }

        // Resize and position
        contentView?.layoutSubtreeIfNeeded()
        let fittingSize = stackView.fittingSize
        let winSize = NSSize(width: max(fittingSize.width + 16, 80),
                             height: fittingSize.height + 12)

        if cursorRect != .zero {
            guard let screen = NSScreen.main else { return }
            var origin = NSPoint(x: cursorRect.minX, y: cursorRect.minY - winSize.height - 4)
            // Keep within screen bounds
            origin.x = max(0, min(origin.x, screen.visibleFrame.maxX - winSize.width))
            origin.y = max(screen.visibleFrame.minY, origin.y)
            setFrame(NSRect(origin: origin, size: winSize), display: true, animate: false)
        }

        orderFront(nil)
    }

    // MARK: – Actions

    @objc private func candidateClicked(_ sender: NSButton) {
        onSelect?(sender.tag)
    }

    @objc private func prevPage() { onPageChange?(true) }
    @objc private func nextPage() { onPageChange?(false) }

    // MARK: – Button factories

    private func makeButton(label: String, comment: String) -> NSButton {
        let btn = NSButton()
        btn.isBordered = false
        btn.bezelStyle = .rounded
        let title = NSMutableAttributedString()
        title.append(NSAttributedString(string: label, attributes: [
            .font: NSFont.systemFont(ofSize: 14),
            .foregroundColor: NSColor.labelColor,
        ]))
        if !comment.isEmpty {
            title.append(NSAttributedString(string: " \(comment)", attributes: [
                .font: NSFont.systemFont(ofSize: 11),
                .foregroundColor: NSColor.secondaryLabelColor,
            ]))
        }
        btn.attributedTitle = title
        btn.contentTintColor = .labelColor
        btn.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        return btn
    }

    private func makeNavButton(symbol: String) -> NSButton {
        let btn = NSButton()
        btn.isBordered = false
        btn.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
        btn.imageScaling = .scaleProportionallyDown
        btn.widthAnchor.constraint(equalToConstant: 20).isActive = true
        return btn
    }
}

// MARK: – Safe array subscript

private extension Array {
    subscript(safe index: Int) -> Element? {
        guard index >= 0 && index < count else { return nil }
        return self[index]
    }
}
