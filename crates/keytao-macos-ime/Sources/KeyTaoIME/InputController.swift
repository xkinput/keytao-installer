import Cocoa
import InputMethodKit
import Carbon
import CKeytaoCore

/// KeyTao's IMKInputController subclass.
/// The OS creates one instance per client text field and routes key events here.
@objc(KeyTaoInputController)
class KeyTaoInputController: IMKInputController {

    private weak var candidatePanel: CandidatePanel?

    // MARK: – Lifecycle

    override init!(server: IMKServer!, delegate: Any!, client: Any!) {
        super.init(server: server, delegate: delegate, client: client)
        ensureEngineReady()
    }

    // MARK: – Key handling

    /// Called for every key event in the client app. Return true to consume it.
    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event else { return false }

        // Pass through modifier-only events
        if event.type == .flagsChanged { return false }
        // Only intercept keyDown events
        guard event.type == .keyDown else { return false }

        let keyval = carbonKeyCode(from: event)
        let modifiers = carbonModifiers(from: event.modifierFlags)

        let statePtr = keytao_process_key(keyval, modifiers)
        defer { keytao_free_state(statePtr) }

        guard let state = statePtr else { return false }

        let preedit  = state.pointee.preedit.map { String(cString: $0) } ?? ""
        let committed = state.pointee.committed.map { String(cString: $0) } ?? ""
        let candidateCount = Int(state.pointee.candidate_count)

        // Commit any pending text first
        if !committed.isEmpty {
            if let client = sender as? IMKTextInput {
                client.insertText(committed, replacementRange: NSRange(location: NSNotFound, length: 0))
            }
            hideCandidates()
            if preedit.isEmpty { return true }
        }

        // Update preedit (inline composition)
        if let client = sender as? IMKTextInput {
            if preedit.isEmpty {
                client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0),
                                     replacementRange: NSRange(location: NSNotFound, length: 0))
                hideCandidates()
            } else {
                let attrs = mark(forStyle: kTSMHiliteSelectedRawText, at: NSRange(location: 0, length: preedit.utf16.count))
                let marked = NSAttributedString(string: preedit, attributes: attrs as? [NSAttributedString.Key: Any])
                client.setMarkedText(marked, selectionRange: NSRange(location: preedit.utf16.count, length: 0),
                                     replacementRange: NSRange(location: NSNotFound, length: 0))
            }
        }

        // Show/update candidate window
        if candidateCount > 0 {
            var texts: [String] = []
            var comments: [String] = []
            for i in 0..<candidateCount {
                let t = state.pointee.candidate_texts?[i].map { String(cString: $0) } ?? ""
                let c = state.pointee.candidate_comments?[i].map { String(cString: $0) } ?? ""
                texts.append(t)
                comments.append(c)
            }
            let page       = Int(state.pointee.page)
            let isLastPage = state.pointee.is_last_page
            let selectKeys = state.pointee.select_keys.map { String(cString: $0) } ?? ""
            showCandidates(texts: texts, comments: comments,
                           page: page, isLastPage: isLastPage,
                           selectKeys: selectKeys,
                           client: sender)
        } else {
            hideCandidates()
        }

        // If there is a preedit we consumed the event; otherwise pass it through
        return !preedit.isEmpty
    }

    // MARK: – Commit / cancel

    override func commitComposition(_ sender: Any!) {
        // Commit by sending Return to the engine (select first candidate or commit raw)
        let statePtr = keytao_process_key(UInt32(kVK_Return), 0)
        defer { keytao_free_state(statePtr) }
        if let state = statePtr {
            let committed = state.pointee.committed.map { String(cString: $0) } ?? ""
            if !committed.isEmpty, let client = sender as? IMKTextInput {
                client.insertText(committed, replacementRange: NSRange(location: NSNotFound, length: 0))
            }
        }
        hideCandidates()
    }

    override func cancelComposition() {
        let statePtr = keytao_reset()
        keytao_free_state(statePtr)
        hideCandidates()
    }

    // MARK: – Candidate window helpers

    private func showCandidates(texts: [String], comments: [String],
                                 page: Int, isLastPage: Bool, selectKeys: String,
                                 client: Any?) {
        let panel: CandidatePanel
        if let existing = candidatePanel {
            panel = existing
        } else {
            panel = CandidatePanel()
            candidatePanel = panel
        }

        var cursorRect = NSRect.zero
        if let client = client as? IMKTextInput {
            var actual = NSRect.zero
            client.attributes(forCharacterIndex: 0, lineHeightRectangle: &actual)
            cursorRect = actual
        }
        panel.update(texts: texts, comments: comments,
                     page: page, isLastPage: isLastPage, selectKeys: selectKeys,
                     near: cursorRect)

        panel.onSelect = { [weak self] index in
            guard let self else { return }
            self.handleCandidateSelection(index: index, client: client)
        }
        panel.onPageChange = { [weak self] backward in
            guard let self else { return }
            self.handlePageChange(backward: backward, client: client)
        }
    }

    private func hideCandidates() {
        candidatePanel?.orderOut(nil)
    }

    private func handleCandidateSelection(index: Int, client: Any?) {
        let statePtr = keytao_select_candidate(UInt32(index))
        defer { keytao_free_state(statePtr) }
        guard let state = statePtr else { return }
        let committed = state.pointee.committed.map { String(cString: $0) } ?? ""
        if !committed.isEmpty, let client = client as? IMKTextInput {
            client.insertText(committed, replacementRange: NSRange(location: NSNotFound, length: 0))
            hideCandidates()
        }
    }

    private func handlePageChange(backward: Bool, client: Any?) {
        let statePtr = keytao_change_page(backward)
        defer { keytao_free_state(statePtr) }
        guard let state = statePtr else { return }
        // Rebuild candidate list after page turn
        let count = Int(state.pointee.candidate_count)
        if count > 0 {
            var texts: [String] = []
            var comments: [String] = []
            for i in 0..<count {
                texts.append(state.pointee.candidate_texts?[i].map { String(cString: $0) } ?? "")
                comments.append(state.pointee.candidate_comments?[i].map { String(cString: $0) } ?? "")
            }
            candidatePanel?.update(
                texts: texts, comments: comments,
                page: Int(state.pointee.page), isLastPage: state.pointee.is_last_page,
                selectKeys: state.pointee.select_keys.map { String(cString: $0) } ?? "",
                near: .zero)
        }
    }

    // MARK: – Key code conversion

    /// Map NSEvent keyCode to X11/Rime key values.
    private func carbonKeyCode(from event: NSEvent) -> UInt32 {
        // Printable characters: use unicode character directly
        if let ch = event.characters?.unicodeScalars.first?.value, ch >= 0x20, ch < 0x7f {
            return ch
        }
        // Special keys (Carbon virtual key codes → X11 keysyms Rime expects)
        switch Int(event.keyCode) {
        case kVK_Return:        return 0xff0d  // XK_Return
        case kVK_Delete:        return 0xff08  // XK_BackSpace
        case kVK_ForwardDelete: return 0xffff  // XK_Delete
        case kVK_Escape:        return 0xff1b  // XK_Escape
        case kVK_Space:         return 0x0020  // space
        case kVK_LeftArrow:     return 0xff51  // XK_Left
        case kVK_RightArrow:    return 0xff53  // XK_Right
        case kVK_UpArrow:       return 0xff52  // XK_Up
        case kVK_DownArrow:     return 0xff54  // XK_Down
        case kVK_Home:          return 0xff50  // XK_Home
        case kVK_End:           return 0xff57  // XK_End
        case kVK_PageUp:        return 0xff55  // XK_Page_Up
        case kVK_PageDown:      return 0xff56  // XK_Page_Down
        case kVK_Tab:           return 0xff09  // XK_Tab
        default:
            if let ch = event.characters?.unicodeScalars.first?.value {
                return ch
            }
            return 0
        }
    }

    /// Convert NSEvent modifier flags to Rime modifier mask.
    private func carbonModifiers(from flags: NSEvent.ModifierFlags) -> UInt32 {
        var mask: UInt32 = 0
        if flags.contains(.shift)   { mask |= 1 }        // RIME_MODIFIER_SHIFT
        if flags.contains(.control) { mask |= 4 }        // RIME_MODIFIER_CONTROL
        if flags.contains(.option)  { mask |= 8 }        // RIME_MODIFIER_ALT
        return mask
    }
}
