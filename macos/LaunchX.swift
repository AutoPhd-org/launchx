// LaunchX.app: a window and a menu-bar menu for the LaunchX launcher.
//
// The app holds no state of its own. It reads the daemon's run file
// (~/.launchx/run/launchx.json) for the API address and token, signs its web
// views in with one-time links, and shows the same pages a browser would.

import AppKit
import SwiftUI
import WebKit

// MARK: - Talking to the daemon

struct RunFile: Decodable {
    let pid: Int32
    let url: String
    let token: String
}

struct ServiceInfo: Decodable, Identifiable, Equatable {
    let id: String
    let name: String
    let status: String
    let health: String?
    let summary: String?
    let detail: String?
    let error: String?
    let ui: String?
    let required_by: [String]

    var active: Bool { ["running", "starting", "waiting"].contains(status) }
}

private struct StateDocument: Decodable {
    let services: [ServiceInfo]
}

enum LaunchXError: LocalizedError {
    case notRunning
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .notRunning: return "LaunchX is not running."
        case .failed(let message): return message
        }
    }
}

enum Daemon {
    static var home: URL {
        if let path = ProcessInfo.processInfo.environment["LAUNCHX_HOME"], !path.isEmpty {
            return URL(fileURLWithPath: path)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".launchx")
    }

    static func runFile() -> RunFile? {
        let url = home.appendingPathComponent("run/launchx.json")
        guard let data = try? Data(contentsOf: url),
              let run = try? JSONDecoder().decode(RunFile.self, from: data) else { return nil }
        return (kill(run.pid, 0) == 0 || errno == EPERM) ? run : nil
    }

    private static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.connectionProxyDictionary = [:]  // the token never goes through a proxy
        configuration.timeoutIntervalForRequest = 180  // stopping waits for services to exit
        return URLSession(configuration: configuration)
    }()

    static func call(_ method: String, _ path: String, body: [String: Any]? = nil) async throws -> Data {
        guard let run = runFile(), let url = URL(string: run.url + path) else { throw LaunchXError.notRunning }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("Bearer \(run.token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            let reply = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            throw LaunchXError.failed(reply?["error"] as? String ?? "LaunchX answered \(status)")
        }
        return data
    }

    static func services() async throws -> [ServiceInfo] {
        try JSONDecoder().decode(StateDocument.self, from: try await call("GET", "/api/state")).services
    }

    static func act(_ id: String, _ action: String) async throws {
        _ = try await call("POST", "/api/services/\(id)/\(action)", body: [:])
    }

    /// A one-time link that signs a browser in, landing on the launcher or on a service's `next` path.
    static func signInLink(service: String? = nil, next: String = "/") async throws -> URL {
        var body: [String: Any] = ["next": next]
        if let service { body["service"] = service }
        let data = try await call("POST", "/api/link", body: body)
        let reply = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let text = reply?["url"] as? String, let url = URL(string: text) else {
            throw LaunchXError.failed("LaunchX returned no sign-in link.")
        }
        return url
    }

    /// The `lx` program: the copy inside this app, else ~/.local/bin/lx.
    static var lx: URL? {
        let bundled = Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/lx")
        if FileManager.default.isExecutableFile(atPath: bundled.path) { return bundled }
        let installed = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/lx")
        return FileManager.default.isExecutableFile(atPath: installed.path) ? installed : nil
    }

    /// `lx up`: start the daemon now and at every login.
    static func startDaemon() async throws {
        guard let lx else { throw LaunchXError.failed("Cannot find the lx program.") }
        try await Task.detached {
            let process = Process()
            process.executableURL = lx
            process.arguments = ["up"]
            let pipe = Pipe()
            process.standardOutput = pipe
            process.standardError = pipe
            try process.run()
            let output = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            if process.terminationStatus != 0 {
                let text = String(data: output, encoding: .utf8) ?? ""
                throw LaunchXError.failed(text.trimmingCharacters(in: .whitespacesAndNewlines))
            }
        }.value
    }
}

func showError(_ error: Error) {
    let alert = NSAlert()
    alert.alertStyle = .warning
    alert.messageText = "LaunchX"
    alert.informativeText = error.localizedDescription
    alert.runModal()
}

// MARK: - Model shared by the menu and the windows

@MainActor
final class Model: ObservableObject {
    static let shared = Model()

    @Published var services: [ServiceInfo] = []
    @Published var daemonRunning = Daemon.runFile() != nil
    @Published var startingDaemon = false
    private var timer: Timer?

    func begin() {
        Task { await refresh() }
        timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
            Task { @MainActor in await Model.shared.refresh() }
        }
    }

    func refresh() async {
        do {
            let list = try await Daemon.services()
            if list != services { services = list }
            if !daemonRunning { daemonRunning = true }
        } catch {
            let running = Daemon.runFile() != nil
            if running != daemonRunning { daemonRunning = running }
            if !running && !services.isEmpty { services = [] }
        }
    }

    func perform(_ service: ServiceInfo, _ action: String) {
        if action == "stop" {
            let dependents = service.required_by.filter { other in services.first { $0.id == other }?.active ?? false }
            if !dependents.isEmpty {
                let alert = NSAlert()
                alert.messageText = "Stop \(service.name)?"
                alert.informativeText = "\(dependents.joined(separator: ", ")) requires it and will stop first."
                alert.addButton(withTitle: "Stop")
                alert.addButton(withTitle: "Cancel")
                guard alert.runModal() == .alertFirstButtonReturn else { return }
            }
        }
        Task {
            do { try await Daemon.act(service.id, action) } catch { showError(error) }
            await refresh()
        }
    }

    /// Open a service's page in the default browser, starting it first if needed.
    /// The gateway shows a "starting" page that reloads itself once the service is up.
    func open(_ service: ServiceInfo) {
        Task {
            if !service.active {
                do { try await Daemon.act(service.id, "start") } catch { showError(error); return }
            }
            await openInBrowser(service: service.id)
        }
    }
}

/// Hand a one-time sign-in link to the default browser, so the tab opens signed in.
@MainActor
func openInBrowser(service: String, path: String = "/") async {
    do {
        NSWorkspace.shared.open(try await Daemon.signInLink(service: service, next: path))
    } catch {
        showError(error)
    }
}

extension Model {

    func startDaemon() {
        guard !startingDaemon else { return }
        startingDaemon = true
        Task {
            defer { startingDaemon = false }
            do {
                try await Daemon.startDaemon()
                await refresh()
                Windows.shared.showLauncher()
            } catch {
                showError(error)
            }
        }
    }
}

// MARK: - Windows

@MainActor
final class Windows: NSObject, NSWindowDelegate, WKUIDelegate, WKNavigationDelegate {
    static let shared = Windows()

    private var windows: [String: NSWindow] = [:]
    private var titleObservers: [String: NSKeyValueObservation] = [:]
    private lazy var configuration: WKWebViewConfiguration = {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()  // keeps sessions between launches
        return configuration
    }()

    func showLauncher() {
        Task {
            guard Daemon.runFile() != nil else { showOffline(); return }
            if let window = windows["launcher"], let web = window.contentView as? WKWebView, web.url?.host == "launchx.localhost" {
                bringForward(window)
                return
            }
            do {
                let link = try await Daemon.signInLink()
                open(key: "launcher", url: link, title: "LaunchX", size: NSSize(width: 1040, height: 720))
            } catch {
                showOffline()
            }
        }
    }

    func reloadFrontWindow() {
        (NSApp.keyWindow?.contentView as? WKWebView)?.reload()
    }

    private func showOffline() {
        let window = windows["launcher"] ?? makeWindow(key: "launcher", title: "LaunchX", size: NSSize(width: 1040, height: 720))
        if !(window.contentView is NSHostingView<OfflineView>) {
            window.contentView = NSHostingView(rootView: OfflineView())
        }
        bringForward(window)
    }

    private func open(key: String, url: URL, title: String, size: NSSize) {
        let window = windows[key] ?? makeWindow(key: key, title: title, size: size)
        let web = WKWebView(frame: window.contentView?.bounds ?? NSRect(origin: .zero, size: size), configuration: configuration)
        web.uiDelegate = self
        web.navigationDelegate = self
        web.allowsBackForwardNavigationGestures = true
        window.contentView = web
        titleObservers[key] = web.observe(\.title, options: [.new]) { [weak window] web, _ in
            Task { @MainActor in
                if let title = web.title, !title.isEmpty { window?.title = title }
            }
        }
        web.load(URLRequest(url: url))
        bringForward(window)
    }

    private func makeWindow(key: String, title: String, size: NSSize) -> NSWindow {
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.minSize = NSSize(width: 420, height: 320)
        window.center()
        window.setFrameAutosaveName("LaunchX.window.\(key)")
        window.tabbingMode = .disallowed
        windows[key] = window
        return window
    }

    private func bringForward(_ window: NSWindow) {
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func windowWillClose(_ notification: Notification) {
        guard let window = notification.object as? NSWindow,
              let key = windows.first(where: { $0.value === window })?.key else { return }
        (window.contentView as? WKWebView)?.stopLoading()
        window.contentView = nil
        windows[key] = nil
        titleObservers[key] = nil
    }

    // MARK: Links

    private func serviceID(_ url: URL) -> String? {
        guard let host = url.host, host.hasSuffix(".localhost"), host != "launchx.localhost" else { return nil }
        return String(host.dropLast(".localhost".count))
    }

    private func pathAndQuery(_ url: URL) -> String {
        let parts = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let path = parts?.percentEncodedPath.isEmpty == false ? parts!.percentEncodedPath : "/"
        return parts?.percentEncodedQuery.map { "\(path)?\($0)" } ?? path
    }

    /// The window shows only the launcher. A service's page goes to the default browser,
    /// signed in; any other address goes there as it is.
    private func route(_ url: URL) {
        if let id = serviceID(url) {
            let path = pathAndQuery(url)
            Task { await openInBrowser(service: id, path: path) }
        } else if url.host == "launchx.localhost" {
            showLauncher()
        } else {
            NSWorkspace.shared.open(url)
        }
    }

    /// `target="_blank"` links, such as a card's Open button.
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = navigationAction.request.url { route(url) }
        return nil
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let url = navigationAction.request.url else { return .allow }
        if url.host == "launchx.localhost" || ["about", "blob", "data"].contains(url.scheme ?? "") { return .allow }
        route(url)
        return .cancel
    }

    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo) async {
        let alert = NSAlert()
        alert.messageText = message
        alert.runModal()
    }

    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo) async -> Bool {
        let alert = NSAlert()
        alert.messageText = message
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        return alert.runModal() == .alertFirstButtonReturn
    }

    /// Downloads and file links the web view cannot show go to the default browser.
    func webView(_ webView: WKWebView, decidePolicyFor navigationResponse: WKNavigationResponse) async -> WKNavigationResponsePolicy {
        if navigationResponse.canShowMIMEType { return .allow }
        if let url = navigationResponse.response.url { NSWorkspace.shared.open(url) }
        return .cancel
    }
}

// MARK: - Views

struct OfflineView: View {
    @ObservedObject private var model = Model.shared

    var body: some View {
        VStack(spacing: 14) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 84, height: 84)
            Text("LaunchX is not running")
                .font(.title2.weight(.semibold))
            Text("Start it to see and control your services.\nIt will also start at every login.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Button(model.startingDaemon ? "Starting…" : "Start LaunchX") { model.startDaemon() }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(model.startingDaemon)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private func dot(_ service: ServiceInfo) -> String {
    switch service.status {
    case "running":
        switch service.health {
        case "degraded": return "🟡"
        case "unhealthy": return "🔴"
        default: return "🟢"
        }
    case "starting", "waiting", "stopping": return "🟡"
    case "failed": return "🔴"
    default: return "⚪️"
    }
}

private func statusLine(_ service: ServiceInfo) -> String {
    let status = service.status.prefix(1).uppercased() + service.status.dropFirst()
    if let problem = service.error ?? (service.health == "unhealthy" ? service.detail : nil) {
        return "\(status) — \(problem)"
    }
    if let summary = service.summary ?? service.detail { return "\(status) — \(summary)" }
    return status
}

struct MenuContent: View {
    @ObservedObject var model: Model

    var body: some View {
        if model.daemonRunning {
            if model.services.isEmpty {
                Text("No services registered")
            }
            ForEach(model.services) { service in
                Menu {
                    Text(statusLine(service))
                    Divider()
                    if service.ui != nil {
                        Button("Open") { model.open(service) }
                    }
                    if service.active {
                        Button("Stop") { model.perform(service, "stop") }
                    } else {
                        Button("Start") { model.perform(service, "start") }
                    }
                    if service.status == "running" {
                        Button("Restart") { model.perform(service, "restart") }
                    }
                } label: {
                    Text("\(dot(service))  \(service.name)")
                }
            }
            Divider()
            Button("Open LaunchX") { Windows.shared.showLauncher() }
                .keyboardShortcut("l")
        } else {
            Text("LaunchX is not running")
            Button(model.startingDaemon ? "Starting…" : "Start LaunchX") { model.startDaemon() }
                .disabled(model.startingDaemon)
        }
        Divider()
        Button("Quit LaunchX App (services keep running)") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }
}

// MARK: - App

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { @MainActor in
            Model.shared.begin()
            Windows.shared.showLauncher()
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { Task { @MainActor in Windows.shared.showLauncher() } }
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}

@main
struct LaunchXApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @ObservedObject private var model = Model.shared

    var body: some Scene {
        MenuBarExtra {
            MenuContent(model: model)
        } label: {
            Image(systemName: "paperplane.fill")
        }
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Open LaunchX") { Windows.shared.showLauncher() }
                    .keyboardShortcut("l")
            }
            CommandGroup(before: .toolbar) {
                Button("Reload Page") { Windows.shared.reloadFrontWindow() }
                    .keyboardShortcut("r")
                Divider()
            }
        }
    }
}
