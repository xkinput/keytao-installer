import Cocoa
import InputMethodKit
import CKeytaoCore

/// Called once when the OS first launches (or reactivates) the input method process.
func initializeEngine() {
    let home = FileManager.default.homeDirectoryForCurrentUser.path
    let userDir = (home as NSString).appendingPathComponent("Library/keytao")
    let sharedDir = resolveSharedDataDir()

    let ok = keytao_init(userDir, sharedDir)
    if ok {
        NSLog("KeyTao: engine initialized (user=%@, shared=%@)", userDir, sharedDir)
    } else {
        NSLog("KeyTao: engine initialization FAILED")
    }
}

/// Finds the best shared rime-data directory available on this machine.
func resolveSharedDataDir() -> String {
    let candidates = [
        "/Library/Input Methods/Squirrel.app/Contents/SharedSupport",
        "/opt/homebrew/share/rime-data",
        "/usr/local/share/rime-data",
    ]
    for path in candidates {
        if FileManager.default.fileExists(atPath: path) {
            return path
        }
    }
    return ""
}

// Initialize the engine as soon as the dynamic library is loaded
// (which happens when the IMK server first creates a controller).
// We use a dispatch_once pattern via a lazy global.
private var _engineInitialized: Bool = {
    initializeEngine()
    return true
}()

func ensureEngineReady() {
    _ = _engineInitialized
}
