// Claude Code Notifier.app: posts ccn's desktop notifications on macOS, so they carry our name and icon
// instead of Script Editor's. ccn runs the executable inside the bundle directly:
//
//   ccn-notifier --title T --body B [--sound]   post one notification, exit when it is delivered
//                                               (through AppleScript until permission has been asked)
//   ccn-notifier --request-permission           ask for permission now (ccn setup); prints granted|denied
//   ccn-notifier --status                       prints authorized|denied|not-determined (ccn doctor)
//
// Launched with no arguments (macOS does that when a notification is clicked) it just quits.

import Foundation
import UserNotifications

let center = UNUserNotificationCenter.current()
let args = CommandLine.arguments.dropFirst()

func value(_ flag: String) -> String? {
    guard let i = args.firstIndex(of: flag), args.index(after: i) < args.endIndex else { return nil }
    return args[args.index(after: i)]
}

func finish(_ line: String? = nil, code: Int32 = 0) -> Never {
    if let line { print(line) }
    exit(code)
}

// Never outlive the hook by much, even if the notification server doesn't answer
DispatchQueue.main.asyncAfter(deadline: .now() + 60) { finish(code: 2) }

final class Delegate: NSObject, UNUserNotificationCenterDelegate {
    // Show banners even though this app counts as frontmost while it runs
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        completionHandler()
        finish()
    }
}
let delegate = Delegate()
center.delegate = delegate

func appleScript(_ content: UNNotificationContent) -> Never {
    func quoted(_ s: String) -> String {
        "\"" + s.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
    }
    var script = "display notification \(quoted(content.body)) with title \(quoted(content.title))"
    if content.sound != nil { script += " sound name \"default\"" }
    let osascript = Process()
    osascript.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    osascript.arguments = ["-e", script]
    try? osascript.run()
    osascript.waitUntilExit()
    finish()
}

func authorize(_ then: @escaping (Bool) -> Void) {
    center.requestAuthorization(options: [.alert, .sound]) { granted, error in
        if let error { FileHandle.standardError.write(Data("\(error)\n".utf8)) }
        then(granted)
    }
}

if args.contains("--status") {
    center.getNotificationSettings { settings in
        switch settings.authorizationStatus {
        case .authorized, .provisional: finish("authorized")
        case .denied: finish("denied")
        default: finish("not-determined")
        }
    }
} else if args.contains("--request-permission") {
    authorize { granted in finish(granted ? "granted" : "denied") }
} else if let title = value("--title") {
    let content = UNMutableNotificationContent()
    content.title = title
    content.body = value("--body") ?? ""
    if args.contains("--sound") { content.sound = .default }
    // One thread per Claude Code session keeps Notification Center tidy
    if let thread = value("--thread") { content.threadIdentifier = thread }

    // macOS only shows the permission prompt to an app opened through Launch Services, which
    // `ccn setup` does. Until it has asked, post through AppleScript so nothing is lost; once the
    // user has turned us off, stay quiet.
    center.getNotificationSettings { settings in
        switch settings.authorizationStatus {
        case .authorized, .provisional:
            center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)) { error in
                finish(code: error == nil ? 0 : 1)
            }
        case .denied:
            finish(code: 1)
        default:
            appleScript(content)
        }
    }
} else {
    // Opened by a click on one of our notifications: nothing to do yet
    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { finish() }
}

RunLoop.main.run()
