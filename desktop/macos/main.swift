import AppKit
import UniformTypeIdentifiers

// The app is a thin native client of the bundled, versioned CLI. All project
// validation, downloads, editor integration and builds remain in texe.
final class Welcome: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 760, height: 700),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
    let name = NSTextField(string: "my-paper")
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
        window.minSize = NSSize(width: 700, height: 700)
        let content = window.contentView!
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -24),
            stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 24),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -24)
        ])
        let heading = NSTextField(labelWithString: "Your next paper starts here.")
        heading.font = .systemFont(ofSize: 25, weight: .semibold)
        stack.addArrangedSubview(heading)
        stack.addArrangedSubview(NSTextField(wrappingLabelWithString:
            "Write locally. texe installs the LaTeX tools and packages your paper needs. The first build needs internet and may take several minutes."))
        for (label, field) in [("Folder name", name), ("Paper title", title), ("Author", author)] {
            let caption = NSTextField(labelWithString: label)
            caption.widthAnchor.constraint(equalToConstant: 100).isActive = true
            let row = NSStackView(views: [caption, field])
            field.widthAnchor.constraint(greaterThanOrEqualToConstant: 360).isActive = true
            field.setAccessibilityLabel(label)
            stack.addArrangedSubview(row)
        }
        editor.addItems(withTitles: ["VS Code", "My own editor + browser preview"])
        editor.setAccessibilityLabel("Editor")
        engine.addItems(withTitles: ["pdfLaTeX", "LuaLaTeX"])
        engine.setAccessibilityLabel("LaTeX engine for new or unconfigured projects")
        stack.addArrangedSubview(NSStackView(views: [NSTextField(labelWithString: "Editor"), editor,
            NSTextField(labelWithString: "Engine"), engine]))
        let buttons = NSStackView()
        for (label, action) in [("Create a paper…", #selector(create)), ("Open a paper…", #selector(open)),
                                ("Build again", #selector(build)), ("Show files", #selector(showFiles))] {
            let button = NSButton(title: label, target: self, action: action)
            actions.append(button)
            buttons.addArrangedSubview(button)
        }
        stack.addArrangedSubview(buttons)
        progress.style = .spinning
        progress.isDisplayedWhenStopped = false
        stack.addArrangedSubview(NSStackView(views: [progress, status]))
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        log.frame = NSRect(x: 0, y: 0, width: 650, height: 200)
        log.isVerticallyResizable = true
        log.isHorizontallyResizable = false
        log.textContainer?.widthTracksTextView = true
        log.isEditable = false
        log.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        log.autoresizingMask = [.width]
        log.setAccessibilityLabel("Setup and build details")
        scroll.documentView = log
        stack.addArrangedSubview(scroll)
        scroll.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 200).isActive = true
        actions[2].isEnabled = false
        actions[3].isEnabled = false
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        if let index = CommandLine.arguments.firstIndex(of: "--screenshot"), index + 1 < CommandLine.arguments.count {
            let destination = URL(fileURLWithPath: CommandLine.arguments[index + 1])
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                content.layoutSubtreeIfNeeded()
                guard let bitmap = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { exit(1) }
                content.cacheDisplay(in: content.bounds, to: bitmap)
                guard let png = bitmap.representation(using: .png, properties: [:]) else { exit(1) }
                do { try png.write(to: destination, options: .atomic) }
                catch { fputs("Screenshot failed: \(error)\n", stderr); exit(1) }
                NSApp.terminate(nil)
            }
        }
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
        let folder = name.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !folder.isEmpty, folder != ".", folder != "..", !folder.contains("/"), !folder.contains(":") else {
            alert("Choose a simple folder name without slashes or colons."); return
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
        let previousWatcher = watcher
        watcher = nil
        project = root
        busy = true
        for action in actions { action.isEnabled = false }
        editor.isEnabled = false
        engine.isEnabled = false
        status.stringValue = "Setting up and building your paper…"
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
                    self.finish(useCode ? "PDF ready. Editor setup details are below." : "Paper built. You can start writing.")
                    if !useCode { self.startWatcher(root); self.showFiles() }
                }
            } catch {
                self.append("\n\(error.localizedDescription)\n")
                DispatchQueue.main.async { self.finish("Needs attention — see the details below.") }
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
