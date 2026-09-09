import AppKit

// Compiled only into the disposable runner's acceptance-test app. Uses the real
// UI actions, downloads, bundled CLI, editor and filesystem; no mocked services.
final class MacWorkflowCheck {
    let app: Welcome
    let evidence = URL(fileURLWithPath: ProcessInfo.processInfo.environment["TEXE_WORKFLOW_EVIDENCE"]!)
    let parent = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Documents/texe workflow " + UUID().uuidString)
    var first: URL!
    var second: URL!
    var stage = 0
    var ticks = 0
    var timer: Timer?
    var notes: [String] = []

    init(_ app: Welcome) { self.app = app }
    func require(_ condition: Bool, _ message: String) {
        if !condition { finish(false, message) }
    }
    func note(_ message: String) { notes.append(message); print(message) }
    func capture(_ name: String) {
        guard let content = app.window.contentView else { return }
        content.layoutSubtreeIfNeeded()
        guard let bitmap = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { return }
        content.cacheDisplay(in: content.bounds, to: bitmap)
        try? bitmap.representation(using: .png, properties: [:])?.write(to: evidence.appendingPathComponent(name + ".png"))
    }
    func finish(_ success: Bool, _ message: String) -> Never {
        timer?.invalidate()
        note(message)
        capture(success ? "finished" : "failure")
        try? app.log.string.write(to: evidence.appendingPathComponent("build-details.txt"), atomically: true, encoding: .utf8)
        try? notes.joined(separator: "\n").write(to: evidence.appendingPathComponent("result.txt"), atomically: true, encoding: .utf8)
        exit(success ? 0 : 1)
    }
    func start() {
        require(!app.codeAvailable, "Test requires VS Code to be absent initially")
        require(app.editor.indexOfSelectedItem == 0 && app.engine.indexOfSelectedItem == 0, "Default editor and engine")
        require(!app.window.styleMask.contains(.resizable) && app.destination.isSelectable, "Compact window and selectable destination")
        try! FileManager.default.createDirectory(at: parent, withIntermediateDirectories: true)
        app.actions[4].performClick(nil)
        app.title.stringValue = "Å first paper"
        app.author.stringValue = "Mac acceptance test"
        // Exercise NSOpenPanel itself, including its modal return value.
        choosePanel(parent)
        app.chooseLocation()
        require(app.parentFolder.path == parent.path, "System folder picker returns the chosen parent")
        first = parent.appendingPathComponent("Å first paper")
        require(app.destination.stringValue == first.path, "Visible destination is a project subfolder")
        capture("new-paper")
        app.actions[0].performClick(nil)
        require(!app.codeSetup.isHidden && !app.busy, "Missing editor opens actionable installation screen")
        require(!FileManager.default.fileExists(atPath: first.path), "Choosing a folder does not create project files")
        capture("install-vscode")
        app.installCode.performClick(nil)
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in self.tick() }
    }
    func choosePanel(_ folder: URL) {
        let selector = Timer(timeInterval: 0.5, repeats: true) { timer in
            guard let panel = NSApp.modalWindow as? NSOpenPanel else { return }
            panel.directoryURL = folder
            timer.invalidate()
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { panel.ok(nil) }
        }
        RunLoop.main.add(selector, forMode: .modalPanel)
    }
    func ready(_ root: URL) {
        require(app.activityTitle.stringValue == "Your paper is ready.", "Ready state: " + app.status.stringValue + " / " + app.codeStatus.stringValue)
        require(!app.progress.isDisplayedWhenStopped, "Idle progress indicator is configured to disappear")
        require(app.actions[7].isEnabled && app.openCodeButton.isEnabled, "Completion actions are enabled")
        let pdf = root.appendingPathComponent("main.pdf")
        require((try? Data(contentsOf: pdf).starts(with: Data("%PDF".utf8))) == true, "A real PDF was built")
        require(!FileManager.default.fileExists(atPath: parent.appendingPathComponent("main.tex").path), "Parent folder contains no loose paper files")
        require(NSWorkspace.shared.runningApplications.contains { $0.bundleIdentifier == "com.microsoft.VSCode" }, "VS Code is running while texe reports completion")
    }
    func tick() {
        ticks += 1
        if ticks > 900 { finish(false, "Timed out at stage \(stage): \(app.status.stringValue) / \(app.codeStatus.stringValue)") }
        if app.busy { return }
        switch stage {
        case 0:
            ready(first)
            require(app.codeAvailable, "VS Code installation is detected without restarting texe")
            capture("first-paper-ready")
            note("PASS: native folder picker, guided VS Code installation, first PDF, editor launch and loading completion")
            app.actions[7].performClick(nil)
            require(!app.setup.isHidden, "New paper action opens setup")
            app.title.stringValue = "Second paper"
            app.updateDestination()
            second = parent.appendingPathComponent("Second paper")
            capture("second-paper-setup")
            stage = 1
            app.actions[0].performClick(nil)
        case 1:
            ready(second)
            note("PASS: a second paper gets a separate folder and opens successfully")
            stage = 2
            app.actions[2].performClick(nil)
        case 2:
            ready(second)
            note("PASS: Build again completes while VS Code remains open")
            stage = 3
            app.actions[6].performClick(nil)
            require(!app.home.isHidden, "Your papers returns home")
            choosePanel(first)
            app.actions[1].performClick(nil)
        default:
            ready(first)
            require(app.project?.path == first.path, "Open a paper selects the existing project")
            note("PASS: reopen an existing paper through the native folder picker")
            finish(true, "PASS: complete macOS workflow")
        }
    }
}
