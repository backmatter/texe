import AppKit
import CryptoKit
import UniformTypeIdentifiers

final class WelcomeContent: NSView {
    var fill = NSColor(calibratedRed: 0.992, green: 0.988, blue: 0.980, alpha: 1)
    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        fill.setFill()
        dirtyRect.intersection(bounds).fill()
    }
}

// The app is a thin native client of the bundled, versioned CLI. All project
// validation, downloads, editor integration and builds remain in texe.
final class Welcome: NSObject, NSApplicationDelegate, NSWindowDelegate, NSTextFieldDelegate {
    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 860, height: 600),
                          styleMask: [.titled, .closable, .miniaturizable],
                          backing: .buffered, defer: false)
    let title = NSTextField(string: "My Paper")
    let author = NSTextField(string: "")
    let editor = NSPopUpButton()
    let engine = NSPopUpButton()
    let status = NSTextField(wrappingLabelWithString: "Create a paper or choose an existing project.")
    let progress = NSProgressIndicator()
    let log = NSTextView()
    var actions: [NSButton] = []
    var project: URL?
    var parentFolder = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    let destination = NSTextField(labelWithString: "")
    var watcher: Process?
    var busy = false
    var cli: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/texe") }

    func applicationDidFinishLaunching(_ notification: Notification) {
        window.title = "texe"
        window.delegate = self
        let menu = NSMenu()
        let appItem = NSMenuItem()
        menu.addItem(appItem)
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit texe", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        let editItem = NSMenuItem()
        menu.addItem(editItem)
        let editMenu = NSMenu(title: "Edit")
        for (label, action, key) in [("Cut", "cut:", "x"), ("Copy", "copy:", "c"), ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a")] {
            editMenu.addItem(withTitle: label, action: Selector(action), keyEquivalent: key)
        }
        editItem.submenu = editMenu
        NSApp.mainMenu = menu
        window.contentMinSize = NSSize(width: 860, height: 600)
        window.contentMaxSize = NSSize(width: 860, height: 600)
        window.collectionBehavior = [.fullScreenNone]
        window.standardWindowButton(.zoomButton)?.isEnabled = false
        window.titlebarAppearsTransparent = true
        window.backgroundColor = NSColor(calibratedRed: 0.992, green: 0.988, blue: 0.980, alpha: 1)
        window.appearance = NSAppearance(named: .aqua)
        let content = WelcomeContent(frame: window.contentView!.bounds)
        window.contentView = content
        let sidebar = WelcomeContent(frame: NSRect(x: 0, y: 0, width: 184, height: 600))
        sidebar.fill = NSColor(calibratedRed: 0.957, green: 0.945, blue: 0.933, alpha: 1)
        sidebar.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(sidebar)
        NSLayoutConstraint.activate([
            sidebar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            sidebar.topAnchor.constraint(equalTo: content.topAnchor),
            sidebar.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            sidebar.widthAnchor.constraint(equalToConstant: 184)
        ])
        content.layoutSubtreeIfNeeded()
        let image = NSImageView(frame: NSRect(x: 18, y: 28, width: 137, height: 44))
        image.image = brandImage("logo-wordmark-dark.png")
        image.imageScaling = .scaleProportionallyUpOrDown
        sidebar.addSubview(image)
        let nav = button("Your papers", #selector(goHome), primary: false)
        nav.frame = NSRect(x: 18, y: 100, width: 148, height: 40)
        sidebar.addSubview(nav)
        let foot = label("A little less setup.\nA little more writing.", size: 12, muted: true)
        foot.frame = NSRect(x: 24, y: 510, width: 152, height: 50)

        sidebar.addSubview(foot)
        let workspace = WelcomeContent()
        workspace.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(workspace)
        NSLayoutConstraint.activate([
            workspace.widthAnchor.constraint(equalToConstant: 532),
            workspace.heightAnchor.constraint(equalToConstant: 504),
            workspace.centerXAnchor.constraint(equalTo: content.centerXAnchor, constant: 92),
            workspace.centerYAnchor.constraint(equalTo: content.centerYAnchor)
        ])
        for page in [home, setup, activity, codeSetup] {
            page.frame = NSRect(x: 0, y: 0, width: 532, height: 504)
            workspace.addSubview(page)
        }
        createCodeSetup()
        codeSetup.isHidden = true
        setup.isHidden = true
        activity.isHidden = true
        text(home, "YOUR WORKSPACE", 11, 0, 0, 532, 24, muted: true, bold: true)
        text(home, "Space for your next idea.", 29, 0, 36, 532, 45, bold: true)
        text(home, "Start a paper. Make it yours.", 15, 0, 92, 532, 26, muted: true)
        let card = WelcomeContent(frame: NSRect(x: 0, y: 148, width: 532, height: 144))
        card.fill = NSColor(calibratedRed: 0.957, green: 0.941, blue: 0.929, alpha: 1)
        card.wantsLayer = true
        card.layer?.cornerRadius = 12
        home.addSubview(card)
        let paper = NSImageView(frame: NSRect(x: 28, y: 24, width: 88, height: 96))
        paper.image = NSImage(systemSymbolName: "doc.text", accessibilityDescription: "A new paper")
        paper.contentTintColor = accent
        paper.imageScaling = .scaleProportionallyUpOrDown
        card.addSubview(paper)
        text(card, "Good ideas start here.", 17, 164, 36, 348, 28, bold: true)
        text(card, "A clean page, ready for your words.\nBeautifully typeset from the first draft.", 13, 164, 74, 348, 48, muted: true)
        let newPaper = button("+   New paper", #selector(newPaper), primary: true)
        newPaper.frame = NSRect(x: 0, y: 316, width: 258, height: 44)
        home.addSubview(newPaper)
        let openPaper = button("Open a paper…", #selector(open), primary: false)
        openPaper.frame = NSRect(x: 274, y: 316, width: 258, height: 44)
        home.addSubview(openPaper)
        text(home, "Your files stay on your computer.\ntexe takes care of the tools your paper needs.", 13, 0, 392, 532, 48, muted: true)

        let back = button("←  Your papers", #selector(goHome), primary: false)
        back.frame = NSRect(x: 0, y: 0, width: 160, height: 32)
        setup.addSubview(back)
        text(setup, "Make room for a new paper.", 26, 0, 48, 532, 40, bold: true)
        text(setup, "A title is a good place to start. You can change it later.", 13, 0, 94, 532, 26, muted: true)
        field(setup, "Paper title", title, y: 140, width: 532)
        field(setup, "Author", author, y: 224, width: 532)
        editor.addItems(withTitles: ["VS Code", "My own editor + browser preview"])
        engine.addItems(withTitles: ["pdfLaTeX", "LuaLaTeX"])
        editor.selectItem(at: 0)
        engine.selectItem(at: 0)
        field(setup, "Editor", editor, y: 308, width: 330)
        field(setup, "Typesetting", engine, y: 308, width: 186, x: 346)
        text(setup, "Project folder", 13, 0, 390, 420, 22, muted: true, bold: true)
        destination.frame = NSRect(x: 0, y: 420, width: 416, height: 25)
        destination.isSelectable = true
        destination.lineBreakMode = .byTruncatingMiddle
        destination.setAccessibilityLabel("Final project folder")
        setup.addSubview(destination)
        let browse = button("Change…", #selector(chooseLocation), primary: false)
        browse.frame = NSRect(x: 432, y: 410, width: 100, height: 36)
        setup.addSubview(browse)
        title.delegate = self
        updateDestination()
        let submit = button("Create paper", #selector(create), primary: true)
        submit.frame = NSRect(x: 0, y: 460, width: 220, height: 44)
        setup.addSubview(submit)

        text(activity, "YOUR PAPER", 11, 0, 14, 530, 24, muted: true, bold: true)
        activityTitle = label("", size: 32, bold: true)
        activityTitle.frame = NSRect(x: 0, y: 60, width: 532, height: 108)
        activity.addSubview(activityTitle)
        status.isSelectable = true
        status.font = .systemFont(ofSize: 15)
        status.frame = NSRect(x: 0, y: 199, width: 532, height: 62)
        status.maximumNumberOfLines = 3
        activity.addSubview(status)
        progress.style = .bar
        progress.isIndeterminate = true
        progress.isDisplayedWhenStopped = false
        progress.frame = NSRect(x: 0, y: 278, width: 532, height: 4)
        activity.addSubview(progress)
        activityHint = label("", size: 13, muted: true)
        activityHint.frame = NSRect(x: 0, y: 302, width: 532, height: 52)
        activity.addSubview(activityHint)
        let rebuild = button("Build again", #selector(build), primary: false)
        rebuild.frame = NSRect(x: 152, y: 443, width: 140, height: 32)
        let files = button("Show files", #selector(showFiles), primary: false)
        files.frame = NSRect(x: 324, y: 377, width: 150, height: 42)
        let nextPaper = button("New paper", #selector(startAnotherPaper), primary: true)
        nextPaper.frame = NSRect(x: 0, y: 377, width: 150, height: 42)
        openCodeButton = button("Open in VS Code", #selector(openInCode), primary: false)
        openCodeButton.frame = NSRect(x: 162, y: 377, width: 150, height: 42)
        activity.addSubview(nextPaper)
        activity.addSubview(openCodeButton)
        activity.addSubview(rebuild)
        activity.addSubview(files)
        actions = [submit, openPaper, rebuild, files, newPaper, back, nav, nextPaper, openCodeButton]
        rebuild.isEnabled = false
        files.isEnabled = false
        let details = button("Show details", #selector(showDetails), primary: false)
        details.frame = NSRect(x: 0, y: 443, width: 140, height: 32)
        activity.addSubview(details)
        log.isEditable = false
        log.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        log.isVerticallyResizable = true
        log.isHorizontallyResizable = false
        log.textContainer?.widthTracksTextView = true
        log.autoresizingMask = [.width]
        log.setAccessibilityLabel("Setup and build details")
        NSApp.applicationIconImage = brandImage("web-app-manifest-512x512.png")
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        #if WORKFLOW_TEST
        MacWorkflowCheck(self).start()
        #endif
        if let index = CommandLine.arguments.firstIndex(where: { $0 == "--screenshot" || $0 == "--screenshot-setup" || $0 == "--screenshot-code" }), index + 1 < CommandLine.arguments.count {
            if CommandLine.arguments[index] == "--screenshot-setup" { showPage(setup) }
            if CommandLine.arguments[index] == "--screenshot-code" { showPage(codeSetup) }
            let destination = URL(fileURLWithPath: CommandLine.arguments[index + 1])
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                content.layoutSubtreeIfNeeded()
                print("Native preview layout: content=\(content.frame) sidebar=\(sidebar.frame) workspace=\(workspace.frame) children=\(sidebar.subviews.map { $0.frame })")
                guard let bitmap = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { exit(1) }
                content.cacheDisplay(in: content.bounds, to: bitmap)
                guard let png = bitmap.representation(using: .png, properties: [:]) else { exit(1) }
                do { try png.write(to: destination, options: .atomic) }
                catch { fputs("Screenshot failed: \(error)\n", stderr); exit(1) }
                NSApp.terminate(nil)
            }
        }
    }

    let accent = NSColor(calibratedRed: 0.365, green: 0.275, blue: 0.329, alpha: 1)
    let codeSetup = WelcomeContent()
    let codeStatus = NSTextField(wrappingLabelWithString: "VS Code isn’t installed on this computer.")
    let installCode = NSButton(title: "Install VS Code", target: nil, action: nil)
    let checkCode = NSButton(title: "I’ve installed it", target: nil, action: nil)
    var pendingRoot: URL?
    var pendingPrepare: (() throws -> Void)?
    let home = WelcomeContent()
    let setup = WelcomeContent()
    let activity = WelcomeContent()
    var activityTitle = NSTextField(labelWithString: "")
    var activityHint = NSTextField(labelWithString: "")
    var openCodeButton = NSButton()
    var detailsWindow: NSWindow?

    var codeAvailable: Bool {
        environment()["PATH"]!.split(separator: ":").contains {
            FileManager.default.isExecutableFile(atPath: String($0) + "/code")
        }
    }

    func createCodeSetup() {
        text(codeSetup, "ONE-TIME SETUP", 11, 0, 0, 532, 24, muted: true, bold: true)
        text(codeSetup, "Set up your writing app.", 29, 0, 42, 532, 48, bold: true)
        text(codeSetup, "texe opens your paper in Visual Studio Code.\nInstall it once, then get straight to writing.", 15, 0, 110, 532, 64, muted: true)
        codeStatus.frame = NSRect(x: 0, y: 208, width: 532, height: 92)
        codeStatus.font = .systemFont(ofSize: 14)
        codeStatus.isSelectable = true
        codeSetup.addSubview(codeStatus)
        for (control, x, selector) in [(installCode, CGFloat(0), #selector(installVSCode)),
                                      (checkCode, CGFloat(274), #selector(continueFromCode))] {
            control.target = self
            control.action = selector
            control.bezelStyle = .rounded
            control.controlSize = .large
            control.font = .systemFont(ofSize: 14, weight: .medium)
            control.frame = NSRect(x: x, y: 328, width: 258, height: 44)
            codeSetup.addSubview(control)
        }
        installCode.bezelColor = accent
        installCode.contentTintColor = .white
        let back = button("Back to paper setup", #selector(backFromCode), primary: false)
        back.frame = NSRect(x: 0, y: 410, width: 220, height: 36)
        codeSetup.addSubview(back)
    }
    @objc func backFromCode() { if !busy { showPage(setup) } }
    @objc func continueFromCode() {
        guard codeAvailable else {
            codeStatus.stringValue = "We couldn’t find VS Code yet. Finish installing, then try again."
            return
        }
        let root = pendingRoot
        let prepare = pendingPrepare
        pendingRoot = nil
        pendingPrepare = nil
        if let root = root, let prepare = prepare { start(root, prepare: prepare) }
        else { showPage(setup) }
    }

    func installError(_ message: String) -> NSError {
        NSError(domain: "texe-install", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
    func codeMessage(_ message: String) {
        DispatchQueue.main.async { self.codeStatus.stringValue = message }
    }
    // All network/file work runs off the app's main thread. The semaphore only
    // joins URLSession's background completion with that worker.
    func download(_ url: URL, to destination: URL) throws {
        final class Result: @unchecked Sendable { var error: Error? }
        let result = Result()
        let done = DispatchSemaphore(value: 0)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 60
        configuration.timeoutIntervalForResource = 600
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        session.downloadTask(with: url) { file, response, error in
            defer { done.signal() }
            do {
                if let error = error { throw error }
                guard let file = file, (response as? HTTPURLResponse)?.statusCode == 200 else {
                    throw self.installError("The download did not finish. Check your connection and try again.")
                }
                try FileManager.default.moveItem(at: file, to: destination)
            } catch { result.error = error }
        }.resume()
        done.wait()
        if let error = result.error { throw error }
    }
    func installTool(_ path: String, _ args: [String]) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = args
        process.standardInput = FileHandle.nullDevice
        let output = Pipe()
        process.standardOutput = output
        process.standardError = output
        try process.run()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        if process.terminationStatus != 0 {
            throw installError("VS Code could not be installed. " + String(decoding: data, as: UTF8.self))
        }
    }
    @objc func installVSCode() {
        busy = true
        installCode.isEnabled = false
        checkCode.isEnabled = false
        codeStatus.stringValue = "Finding the latest VS Code download…"
        DispatchQueue.global(qos: .userInitiated).async {
            let files = FileManager.default
            let scratch = files.temporaryDirectory.appendingPathComponent("texe-vscode-" + UUID().uuidString)
            defer { try? files.removeItem(at: scratch) }
            do {
                try files.createDirectory(at: scratch, withIntermediateDirectories: true)
                let metadata = scratch.appendingPathComponent("release.json")
                try self.download(URL(string: "https://update.code.visualstudio.com/api/update/darwin-arm64/stable/latest")!, to: metadata)
                guard let info = try JSONSerialization.jsonObject(with: Data(contentsOf: metadata)) as? [String: Any],
                      let address = info["url"] as? String, let url = URL(string: address), url.scheme == "https",
                      let expected = info["sha256hash"] as? String, expected.count == 64, expected.allSatisfy({ $0.isHexDigit }) else {
                    throw self.installError("The download could not be verified. Please try again.")
                }
                let archive = scratch.appendingPathComponent("VSCode.zip")
                self.codeMessage("Downloading VS Code…")
                try self.download(url, to: archive)
                self.codeMessage("Checking the VS Code download…")
                let handle = try FileHandle(forReadingFrom: archive)
                defer { try? handle.close() }
                var hash = SHA256()
                while let data = try handle.read(upToCount: 1_048_576), !data.isEmpty { hash.update(data: data) }
                let actual = hash.finalize().map { String(format: "%02x", $0) }.joined()
                guard actual == expected.lowercased() else { throw self.installError("The download could not be verified. Please try again.") }
                self.codeMessage("Installing VS Code in your Applications folder…")
                try self.installTool("/usr/bin/ditto", ["-x", "-k", archive.path, scratch.path])
                let app = scratch.appendingPathComponent("Visual Studio Code.app")
                try self.installTool("/usr/bin/codesign", ["--verify", "--deep", "--strict", app.path])
                let applications = files.homeDirectoryForCurrentUser.appendingPathComponent("Applications")
                try files.createDirectory(at: applications, withIntermediateDirectories: true)
                let installed = applications.appendingPathComponent("Visual Studio Code.app")
                guard !files.fileExists(atPath: installed.path) else {
                    throw self.installError("VS Code already exists in your Applications folder. Open it to finish setup, then choose I’ve installed it.")
                }
                try files.moveItem(at: app, to: installed)
                DispatchQueue.main.async {
                    self.busy = false
                    self.installCode.isEnabled = true
                    self.checkCode.isEnabled = true
                    self.continueFromCode()
                }
            } catch {
                DispatchQueue.main.async {
                    self.busy = false
                    self.installCode.isEnabled = true
                    self.checkCode.isEnabled = true
                    self.codeStatus.stringValue = "VS Code couldn’t be installed. " + error.localizedDescription
                }
            }
        }
    }

    func brandImage(_ name: String) -> NSImage? {
        let bundled = Bundle.main.resourceURL?.appendingPathComponent(name)
        let development = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent("desktop/brand/texe").appendingPathComponent(name)
        return (bundled.flatMap { NSImage(contentsOf: $0) }) ?? NSImage(contentsOf: development)
    }

    func label(_ value: String, size: CGFloat, muted: Bool = false, bold: Bool = false) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: value)
        label.isSelectable = true
        label.isEditable = false
        label.font = .systemFont(ofSize: size, weight: bold ? .semibold : .regular)
        label.textColor = muted ? NSColor(calibratedWhite: 0.46, alpha: 1) : NSColor(calibratedWhite: 0.19, alpha: 1)
        return label
    }

    func text(_ parent: NSView, _ value: String, _ size: CGFloat, _ x: CGFloat, _ y: CGFloat,
              _ width: CGFloat, _ height: CGFloat, muted: Bool = false, bold: Bool = false) {
        let view = label(value, size: size, muted: muted, bold: bold)
        view.frame = NSRect(x: x - 2, y: y, width: width + 2, height: height)
        parent.addSubview(view)
    }

    func button(_ title: String, _ action: Selector, primary: Bool) -> NSButton {
        let button = NSButton(title: title, target: self, action: action)
        button.bezelStyle = .rounded
        button.controlSize = .large
        button.font = .systemFont(ofSize: 14, weight: .medium)
        if primary { button.bezelColor = accent; button.contentTintColor = .white }
        return button
    }

    func field(_ parent: NSView, _ caption: String, _ control: NSControl, y: CGFloat, width: CGFloat, x: CGFloat = 0) {
        text(parent, caption, 13, x, y, width, 25, muted: true, bold: true)
        control.frame = NSRect(x: x, y: y + 30, width: width, height: 32)
        control.font = .systemFont(ofSize: 16)
        control.controlSize = .large
        control.setAccessibilityLabel(caption)
        parent.addSubview(control)
    }

    func showPage(_ page: NSView) {
        for view in [home, setup, activity, codeSetup] { view.isHidden = view !== page }
    }
    @objc func goHome() { if !busy { showPage(home) } }
    @objc func startAnotherPaper() {
        guard !busy else { return }
        title.stringValue = "Untitled paper"
        updateDestination()
        newPaper()
    }
    @objc func openInCode() {
        guard !busy, let root = project else { return }
        openCodeButton.isEnabled = false
        DispatchQueue.global(qos: .userInitiated).async {
            do { try self.run(["editor", "--project", root.path]) }
            catch {
                let message = error.localizedDescription
                DispatchQueue.main.async { self.status.stringValue = message }
            }
            DispatchQueue.main.async { self.openCodeButton.isEnabled = true }
        }
    }
    @objc func newPaper() { showPage(setup); window.makeFirstResponder(title) }
    @objc func showDetails() {
        if let detailsWindow = detailsWindow { detailsWindow.makeKeyAndOrderFront(nil); return }
        let details = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 760, height: 420),
                               styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        details.title = "Build details · texe"
        details.isReleasedWhenClosed = false
        let scroll = NSScrollView(frame: details.contentView!.bounds.insetBy(dx: 20, dy: 20))
        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true
        log.frame = scroll.bounds
        scroll.documentView = log
        details.contentView?.addSubview(scroll)
        details.center()
        details.makeKeyAndOrderFront(nil)
        detailsWindow = details
    }

    func append(_ text: String) {
        DispatchQueue.main.async {
            // Bound the live transcript so a long writing session stays small.
            if self.log.string.count > 200_000 { self.log.string = String(self.log.string.suffix(100_000)) }
            self.log.textStorage?.append(NSAttributedString(string: text))
            self.log.scrollToEndOfDocument(nil)
        }
    }

    func environment() -> [String: String] {
        var env = ProcessInfo.processInfo.environment
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let paths = [cli.deletingLastPathComponent().path,
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin",
            "\(home)/Applications/Visual Studio Code.app/Contents/Resources/app/bin",
            "/usr/local/bin", "/opt/homebrew/bin", env["PATH"] ?? "/usr/bin:/bin"]
        env["PATH"] = paths.joined(separator: ":")
        env["NO_COLOR"] = "1"
        return env
    }

    func command(_ args: [String]) -> Process {
        let process = Process()
        process.executableURL = cli
        process.arguments = args
        process.environment = environment()
        process.currentDirectoryURL = FileManager.default.homeDirectoryForCurrentUser
        process.standardInput = FileHandle.nullDevice
        return process
    }

    func run(_ args: [String]) throws {
        let process = command(args)
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        try process.run()
        while true {
            let data = pipe.fileHandleForReading.availableData
            if data.isEmpty { break }
            append(String(decoding: data, as: UTF8.self))
        }
        process.waitUntilExit()
        if process.terminationStatus != 0 {
            throw NSError(domain: "texe", code: Int(process.terminationStatus), userInfo:
                [NSLocalizedDescriptionKey: "This step could not finish. See the details below, fix the issue, then try again. Your source files are kept."])
        }
    }

    func pickFolder(_ message: String) -> URL? {
        let panel = NSOpenPanel()
        panel.message = message
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        return panel.runModal() == .OK ? panel.url : nil
    }

    var projectFolderName: String {
        let name = title.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
        return name.isEmpty ? "Untitled paper" : name
    }
    func updateDestination() {
        destination.stringValue = parentFolder.appendingPathComponent(projectFolderName).path
        destination.toolTip = destination.stringValue
    }
    func controlTextDidChange(_ notification: Notification) { updateDestination() }
    @objc func chooseLocation() {
        if let folder = pickFolder("Choose a parent folder. Your paper gets its own subfolder.") {
            parentFolder = folder
            updateDestination()
        }
    }

    @objc func create() {
        let parent = parentFolder
        let folder = projectFolderName
        guard !folder.isEmpty, folder != ".", folder != "..", !folder.contains("/"), !folder.contains(":") else {
            alert("Add a title for your paper."); return
        }
        let root = parent.appendingPathComponent(folder, isDirectory: true)
        guard !FileManager.default.fileExists(atPath: root.path) else {
            alert("That folder already exists. Choose another name, or use Open a paper."); return
        }
        let args = ["init", root.path, "--yes", "--quiet", "--template", "basic", "--engine", selectedEngine,
                    "--title", title.stringValue, "--author", author.stringValue]
        start(root) { try self.run(args) }
    }

    var selectedEngine: String { engine.indexOfSelectedItem == 1 ? "lualatex" : "pdflatex" }

    @objc func open() {
        guard let root = pickFolder("Choose the folder containing your paper.") else { return }
        var args = ["adopt", root.path, "--yes", "--no-build", "--no-editor"]
        if !FileManager.default.fileExists(atPath: root.appendingPathComponent("texe.toml").path) {
            let panel = NSOpenPanel()
            panel.message = "Choose the main .tex file for this paper."
            panel.directoryURL = root
            panel.allowedContentTypes = [UTType(filenameExtension: "tex") ?? .plainText]
            guard panel.runModal() == .OK, let source = panel.url else { return }
            let prefix = root.standardizedFileURL.path + "/"
            guard source.standardizedFileURL.path.hasPrefix(prefix) else {
                alert("Choose a source file inside the selected paper folder."); return
            }
            args += ["--entry", String(source.standardizedFileURL.path.dropFirst(prefix.count)), "--engine", selectedEngine]
        }
        start(root) { try self.run(args) }
    }

    @objc func build() {
        guard let root = project else { return }
        start(root) {}
    }

    func start(_ root: URL, prepare: @escaping () throws -> Void) {
        if editor.indexOfSelectedItem == 0 && !codeAvailable {
            pendingRoot = root
            pendingPrepare = prepare
            codeStatus.stringValue = "VS Code isn’t installed on this computer."
            showPage(codeSetup)
            return
        }
        showPage(activity)
        let previousWatcher = watcher
        watcher = nil
        project = root
        busy = true
        for action in actions { action.isEnabled = false }
        editor.isEnabled = false
        engine.isEnabled = false
        activityTitle.stringValue = "A little preparation.\nThen it’s all yours."
        activityHint.stringValue = "The first setup can take a few minutes.\nYou can leave this window open while we get things ready."
        status.stringValue = "Getting your paper ready…"
        progress.startAnimation(nil)
        log.string = ""
        let useCode = editor.indexOfSelectedItem == 0
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                if let previous = previousWatcher, previous.isRunning {
                    previous.interrupt()
                    previous.waitUntilExit()
                }
                try prepare()
                try self.run(["build", "--project", root.path, "--yes"])
                if useCode {
                    try self.run(["editor", "--project", root.path])
                }
                DispatchQueue.main.async {
                    self.finish(useCode ? "Your paper is ready in VS Code. If asked, choose Trust to enable live preview." : "Paper built. You can start writing.")
                    if !useCode { self.startWatcher(root); self.showFiles() }
                }
            } catch {
                self.append("\n\(error.localizedDescription)\n")
                DispatchQueue.main.async { self.finish(error.localizedDescription, succeeded: false) }
            }
        }
    }

    func finish(_ message: String, succeeded: Bool = true) {
        activityTitle.stringValue = succeeded ? "Your paper is ready." : "Let’s finish setting up."
        activityHint.stringValue = succeeded ? "Keep writing, or start something new." : "Your source files are kept. You can retry or start another paper."
        busy = false
        status.stringValue = message
        progress.stopAnimation(nil)
        for action in actions { action.isEnabled = true }
        actions[2].isEnabled = project.map { FileManager.default.fileExists(atPath: $0.appendingPathComponent("texe.toml").path) } ?? false
        openCodeButton.isEnabled = codeAvailable && actions[2].isEnabled
        actions[3].isEnabled = project.map { FileManager.default.fileExists(atPath: $0.path) } ?? false
        editor.isEnabled = true
        engine.isEnabled = true
    }

    func startWatcher(_ root: URL) {
        let process = command(["watch", "--view", "--project", root.path, "--yes"])
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil }
            else { self.append(String(decoding: data, as: UTF8.self)) }
        }
        process.terminationHandler = { process in
            DispatchQueue.main.async {
                if self.watcher === process {
                    self.watcher = nil
                    self.status.stringValue = "Live preview stopped. Choose Build again to restart."
                }
            }
        }
        do {
            try process.run()
            watcher = process
            status.stringValue = "Watching saves. Keep texe open for live preview."
        } catch { alert(error.localizedDescription) }
    }

    @objc func showFiles() {
        if let root = project { NSWorkspace.shared.open(root) }
    }

    func alert(_ message: String) {
        let alert = NSAlert()
        alert.messageText = message
        alert.runModal()
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if busy {
            alert("Please wait for the current setup or build to finish before quitting.")
            return .terminateCancel
        }
        if let process = watcher, process.isRunning {
            watcher = nil
            process.interrupt()
            status.stringValue = "Finishing the current build and closing live preview…"
            DispatchQueue.global().async {
                process.waitUntilExit()
                DispatchQueue.main.async { NSApp.reply(toApplicationShouldTerminate: true) }
            }
            return .terminateLater
        }
        return .terminateNow
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if busy {
            alert("Please wait for the current setup or build to finish before closing.")
            return false
        }
        return true
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

// Runs from inside the real app bundle in packaging CI, without showing a window.
func smokeTest() throws -> Int32 {
    let cli = Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/texe")
    let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("texe-smoke-" + UUID().uuidString)
    try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: scratch) }
    let paper = scratch.appendingPathComponent("Å paper ' $ ` &")
    for args in [["--version"], ["init", paper.path, "--yes", "--title", "Å paper", "--author", "A Researcher"],
                 ["editor", "--inspect", "--project", paper.path],
                 ["adopt", paper.path, "--check", "--no-editor", "--yes"]] {
        let process = Process()
        process.executableURL = cli
        process.arguments = args
        process.standardInput = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return process.terminationStatus }
    }
    guard FileManager.default.fileExists(atPath: paper.appendingPathComponent("main.tex").path) else { return 1 }
    return 0
}
if CommandLine.arguments.contains("--smoke-test") { exit(try smokeTest()) }
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = Welcome()
app.delegate = delegate
app.run()
