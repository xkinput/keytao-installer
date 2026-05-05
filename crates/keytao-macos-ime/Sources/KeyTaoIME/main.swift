import Cocoa
import InputMethodKit

let connectionName =
  Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String
  ?? "KeyTao_Connection"

let imkServer = IMKServer(
  name: connectionName,
  bundleIdentifier: Bundle.main.bundleIdentifier ?? "ink.rea.keytao-ime"
)
if imkServer == nil {
  NSLog("KeyTao: IMKServer returned nil — TIS may not have registered yet")
}
_ = imkServer

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.run()
