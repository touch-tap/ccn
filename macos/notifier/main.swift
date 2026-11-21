// Claude Code Notifier.app: posts ccn's desktop notifications on macOS, so they carry our name and icon
// instead of Script Editor's. ccn runs the executable inside the bundle directly:
//
//   ccn-notifier --title T --body B [--sound] [--url U] [--activate ID]
//                                               post one notification, exit when it is delivered
//                                               (through AppleScript until permission has been asked);
//                                               a click on it opens U, or else brings the running app
//                                               with bundle id ID (the session's terminal) to the front
//   ccn-notifier --request-permission           ask for permission now (ccn setup); prints granted|denied
//   ccn-notifier --status                       prints authorized|denied|not-determined (ccn doctor)
//
// Launched with no arguments (macOS does that when a notification is clicked) it opens the
// notification's URL, or brings its app forward, and quits.

import AppKit
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

func run(_ path: String, _ arguments: [String]) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: path)
    process.arguments = arguments
    try? process.run()
    process.waitUntilExit()
}

// A reverse-DNS bundle id, checked as ccn checks it (presence::valid_bundle_id): ASCII letters,
// digits, '.' and '-', starting with a letter or digit, two parts or more and none empty, 155 at most
func validBundleID(_ id: String) -> Bool {
    let allowed = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-")
    let parts = id.split(separator: ".", omittingEmptySubsequences: false)
    return (3...155).contains(id.count) && id.allSatisfy { allowed.contains($0) }
        && id.first.map { $0 != "." && $0 != "-" } == true
        && parts.count >= 2 && parts.allSatisfy { !$0.isEmpty }
}

// Brings the app the session runs in to the front, as a click on its Dock icon would. Only when it
// is running: a click on an old notification should never start a terminal.
func activate(_ id: String) {
    guard validBundleID(id),
          let app = NSRunningApplication.runningApplications(withBundleIdentifier: id).first else { return }
    // Activation is cooperative since macOS 14: the app the user clicked (this one) hands over its turn
    if #available(macOS 14, *) { NSApplication.shared.yieldActivation(to: app) }
    run("/usr/bin/open", ["-b", id])
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
        if response.actionIdentifier == UNNotificationDefaultActionIdentifier {
            let info = response.notification.request.content.userInfo
            if let url = info["url"] as? String, url.hasPrefix("https://") {
                run("/usr/bin/open", [url])
            } else if let id = info["activate"] as? String {
                activate(id)
            }
        }
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
    // AppleScript notifications can't open anything on a click, so the URL goes in the body
    var body = content.body
    if let url = content.userInfo["url"] as? String { body += "\n" + url }
    var script = "display notification \(quoted(body)) with title \(quoted(content.title))"
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
    var info: [String: String] = [:]
    if let url = value("--url") { info["url"] = url }
    if let id = value("--activate"), validBundleID(id) { info["activate"] = id }
    content.userInfo = info

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
    // Launched by a click on one of our notifications. The delegate, set above before launch
    // finishes, gets the response once the app has checked in with NSApplication's run loop, acts on
    // it and quits. Quit anyway if no response comes.
    DispatchQueue.main.asyncAfter(deadline: .now() + 10) { finish() }
    NSApplication.shared.setActivationPolicy(.accessory)
    NSApplication.shared.run()
}

RunLoop.main.run()
