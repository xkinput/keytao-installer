import Cocoa
import InputMethodKit

// Called from C main before NSExtensionMain starts the RunLoop.
// The IMKServer must be initialized to register the Mach port that imklaunchagent connects to.
@_cdecl("keytao_imk_setup")
func keytaoIMKSetup() {
  let name =
    Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String
    ?? "KeyTao_Connection"
  let server = IMKServer(
    name: name,
    bundleIdentifier: Bundle.main.bundleIdentifier ?? "ink.rea.keytao-ime"
  )
  if server == nil {
    NSLog("KeyTao: Warning — IMKServer returned nil")
  }
  // Store in a global to keep alive for the process lifetime.
  KeyTaoGlobals.imkServer = server
}

// Global storage to keep IMKServer alive.
enum KeyTaoGlobals {
  static var imkServer: IMKServer?
}
