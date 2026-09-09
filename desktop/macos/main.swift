import AppKit
import UniformTypeIdentifiers

final class WelcomeContent: NSView {
    var fill = NSColor(calibratedRed: 0.992, green: 0.988, blue: 0.980, alpha: 1)
    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        fill.setFill()
        dirtyRect.fill()
    }
}

// The app is a thin native client of the bundled, versioned CLI. All project
// validation, downloads, editor integration and builds remain in texe.
final class Welcome: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 940, height: 680),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
    let title = NSTextField(string: "My Paper")
    let author = NSTextField(string: "")
    let editor = NSPopUpButton()
    let engine = NSPopUpButton()
    let status = NSTextField(labelWithString: "Create a paper or choose an existing project.")
    let progress = NSProgressIndicator()
    let log = NSTextView()
    var actions: [NSButton] = []
    var project: URL?
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
        window.minSize = NSSize(width: 900, height: 700)
        window.titlebarAppearsTransparent = true
        window.backgroundColor = NSColor(calibratedRed: 0.992, green: 0.988, blue: 0.980, alpha: 1)
        window.appearance = NSAppearance(named: .aqua)
        let content = WelcomeContent(frame: window.contentView!.bounds)
        window.contentView = content
        let sidebar = WelcomeContent(frame: NSRect(x: 0, y: 0, width: 208, height: 680))
        sidebar.fill = NSColor(calibratedRed: 0.957, green: 0.945, blue: 0.933, alpha: 1)
        sidebar.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(sidebar)
        NSLayoutConstraint.activate([
            sidebar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            sidebar.topAnchor.constraint(equalTo: content.topAnchor),
            sidebar.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            sidebar.widthAnchor.constraint(equalToConstant: 208)
        ])
        content.layoutSubtreeIfNeeded()
        let image = NSImageView(frame: NSRect(x: 18, y: 34, width: 137, height: 48))
        image.image = brandImage("logo-wordmark-dark.png")
        image.imageScaling = .scaleProportionallyUpOrDown
        sidebar.addSubview(image)
        let nav = button("Your papers", #selector(goHome), primary: false)
        nav.frame = NSRect(x: 18, y: 120, width: 172, height: 42)
        sidebar.addSubview(nav)
        let foot = label("A little less setup.\nA little more writing.", size: 12, muted: true)
        foot.frame = NSRect(x: 28, y: 564, width: 165, height: 55)

        sidebar.addSubview(foot)
        let workspace = WelcomeContent()
        workspace.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(workspace)
        NSLayoutConstraint.activate([
            workspace.widthAnchor.constraint(equalToConstant: 580),
            workspace.heightAnchor.constraint(equalToConstant: 540),
            workspace.centerXAnchor.constraint(equalTo: content.centerXAnchor, constant: 104),
            workspace.centerYAnchor.constraint(equalTo: content.centerYAnchor)
        ])
        for page in [home, setup, activity] {
            page.frame = NSRect(x: 0, y: 0, width: 580, height: 540)
            workspace.addSubview(page)
        }
        setup.isHidden = true
        activity.isHidden = true
        text(home, "YOUR WORKSPACE", 11, 0, 14, 540, 24, muted: true, bold: true)
        text(home, "Space for your next idea.", 32, 0, 60, 570, 52, bold: true)
        text(home, "Start a paper. Make it yours.", 15, 0, 121, 540, 32, muted: true)
        let card = WelcomeContent(frame: NSRect(x: 0, y: 181, width: 532, height: 156))
        card.fill = NSColor(calibratedRed: 0.957, green: 0.941, blue: 0.929, alpha: 1)
        card.wantsLayer = true
        card.layer?.cornerRadius = 12
        home.addSubview(card)
        let paper = NSImageView(frame: NSRect(x: 32, y: 24, width: 94, height: 108))
        paper.image = NSImage(systemSymbolName: "doc.text", accessibilityDescription: "A new paper")
        paper.contentTintColor = accent
        paper.imageScaling = .scaleProportionallyUpOrDown
        card.addSubview(paper)
        text(card, "Good ideas start here.", 17, 164, 43, 348, 30, bold: true)
        text(card, "A clean page, ready for your words.\nBeautifully typeset from the first draft.", 13, 164, 77, 348, 64, muted: true)
        let newPaper = button("+   New paper", #selector(newPaper), primary: true)
        newPaper.frame = NSRect(x: 0, y: 365, width: 258, height: 50)
        home.addSubview(newPaper)
        let openPaper = button("Open a paper…", #selector(open), primary: false)
        openPaper.frame = NSRect(x: 274, y: 365, width: 258, height: 50)
        home.addSubview(openPaper)
        text(home, "Your files stay on your computer.\ntexe takes care of the tools your paper needs.", 13, 0, 443, 530, 58, muted: true)

        let back = button("←  Your papers", #selector(goHome), primary: false)
        back.frame = NSRect(x: 0, y: 0, width: 160, height: 32)
        setup.addSubview(back)
        text(setup, "Make room for a new paper.", 29, 0, 52, 560, 48, bold: true)
        text(setup, "A title is a good place to start. You can change it later.", 13, 0, 106, 560, 28, muted: true)
        field(setup, "Paper title", title, y: 156, width: 532)
        field(setup, "Author", author, y: 241, width: 532)
        editor.addItems(withTitles: ["VS Code", "My own editor + browser preview"])
        engine.addItems(withTitles: ["pdfLaTeX", "LuaLaTeX"])
        let advanced = button("Writing preferences  ⌄", #selector(togglePreferences), primary: false)
        advanced.frame = NSRect(x: 0, y: 321, width: 230, height: 32)
        setup.addSubview(advanced)
        preferences.frame = NSRect(x: 0, y: 365, width: 532, height: 100)
        field(preferences, "Editor", editor, y: 0, width: 330)
        field(preferences, "Typesetting", engine, y: 0, width: 180, x: 352)
        preferences.isHidden = true
        setup.addSubview(preferences)
        let submit = button("Choose location & create", #selector(create), primary: true)
        submit.frame = NSRect(x: 0, y: 479, width: 280, height: 48)
        setup.addSubview(submit)

        text(activity, "YOUR PAPER", 11, 0, 14, 530, 24, muted: true, bold: true)
        text(activity, "A little preparation.\nThen it’s all yours.", 32, 0, 60, 540, 108, bold: true)
        status.font = .systemFont(ofSize: 15)
        status.frame = NSRect(x: 0, y: 199, width: 532, height: 62)
        status.maximumNumberOfLines = 3
        activity.addSubview(status)
        progress.style = .bar
        progress.isIndeterminate = true
        progress.isDisplayedWhenStopped = false
        progress.frame = NSRect(x: 0, y: 278, width: 532, height: 4)
        activity.addSubview(progress)
        text(activity, "The first setup can take a few minutes.\nYou can leave this window open while we get things ready.", 13, 0, 302, 532, 52, muted: true)
        let rebuild = button("Build again", #selector(build), primary: false)
        rebuild.frame = NSRect(x: 0, y: 377, width: 150, height: 42)
        let files = button("Show files", #selector(showFiles), primary: false)
        files.frame = NSRect(x: 162, y: 377, width: 150, height: 42)
        activity.addSubview(rebuild)
        activity.addSubview(files)
        actions = [submit, openPaper, rebuild, files, newPaper, back, nav, advanced]
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
        if let index = CommandLine.arguments.firstIndex(where: { $0 == "--screenshot" || $0 == "--screenshot-setup" }), index + 1 < CommandLine.arguments.count {
            if CommandLine.arguments[index] == "--screenshot-setup" { showPage(setup) }
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
    let home = WelcomeContent()
    let setup = WelcomeContent()
    let activity = WelcomeContent()
    let preferences = WelcomeContent()
    var detailsWindow: NSWindow?

    func brandImage(_ name: String) -> NSImage? {
        let bundled = Bundle.main.resourceURL?.appendingPathComponent(name)
        let development = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent("desktop/brand/texe").appendingPathComponent(name)
        return (bundled.flatMap { NSImage(contentsOf: $0) }) ?? NSImage(contentsOf: development)
    }

    func label(_ value: String, size: CGFloat, muted: Bool = false, bold: Bool = false) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: value)
        label.font = .systemFont(ofSize: size, weight: bold ? .semibold : .regular)
        label.textColor = muted ? NSColor(calibratedWhite: 0.46, alpha: 1) : NSColor(calibratedWhite: 0.19, alpha: 1)
        return label
    }

    func text(_ parent: NSView, _ value: String, _ size: CGFloat, _ x: CGFloat, _ y: CGFloat,
              _ width: CGFloat, _ height: CGFloat, muted: Bool = false, bold: Bool = false) {
        let view = label(value, size: size, muted: muted, bold: bold)
        view.frame = NSRect(x: x, y: y, width: width, height: height)
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
        for view in [home, setup, activity] { view.isHidden = view !== page }
    }
    @objc func goHome() { if !busy { showPage(home) } }
    @objc func newPaper() { showPage(setup); window.makeFirstResponder(title) }
    @objc func togglePreferences() { preferences.isHidden.toggle() }
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

    @objc func create() {
        guard let parent = pickFolder("Choose where to create your paper folder.") else { return }
        let folder = title.stringValue.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
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
        showPage(activity)
        let previousWatcher = watcher
        watcher = nil
        project = root
        busy = true
        for action in actions { action.isEnabled = false }
        editor.isEnabled = false
        engine.isEnabled = false
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
                    guard self.environment()["PATH"]!.split(separator: ":").contains(where: {
                        FileManager.default.isExecutableFile(atPath: String($0) + "/code")
                    }) else {
                        throw NSError(domain: "texe", code: 1, userInfo: [NSLocalizedDescriptionKey:
                            "Your PDF is ready. Install VS Code in Applications, or choose My own editor and Build again."])
                    }
                    try self.run(["editor", "--project", root.path])
                }
                DispatchQueue.main.async {
                    self.finish(useCode ? "Your PDF is ready. Open Show details for editor setup." : "Paper built. You can start writing.")
                    if !useCode { self.startWatcher(root); self.showFiles() }
                }
            } catch {
                self.append("\n\(error.localizedDescription)\n")
                DispatchQueue.main.async { self.finish("Something needs your attention. Open Show details for help.") }
            }
        }
    }

    func finish(_ message: String) {
        busy = false
        status.stringValue = message
        progress.stopAnimation(nil)
        for action in actions { action.isEnabled = true }
        actions[2].isEnabled = project.map { FileManager.default.fileExists(atPath: $0.appendingPathComponent("texe.toml").path) } ?? false
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
