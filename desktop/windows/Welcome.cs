using System;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading.Tasks;
using System.Windows.Forms;

// Uses the .NET Framework supplied with Windows. No PowerShell or separately
// installed desktop runtime is needed on the user's computer.
internal sealed class Welcome : Form
{
    readonly TextBox folderName = new TextBox { Text = "my-paper", Width = 350 };
    readonly TextBox paperTitle = new TextBox { Text = "My Paper", Width = 350 };
    readonly TextBox author = new TextBox { Width = 350 };
    readonly ComboBox editor = new ComboBox { DropDownStyle = ComboBoxStyle.DropDownList, Width = 280 };
    readonly ComboBox engine = new ComboBox { DropDownStyle = ComboBoxStyle.DropDownList, Width = 120 };
    readonly Label status = new Label { AutoSize = true, Text = "Create a paper or choose an existing project." };
    readonly ProgressBar progress = new ProgressBar { Style = ProgressBarStyle.Marquee, Visible = false, Width = 110 };
    readonly TextBox log = new TextBox { Multiline = true, ReadOnly = true, ScrollBars = ScrollBars.Vertical,
        Dock = DockStyle.Fill, Font = new Font("Consolas", 9), AccessibleName = "Setup and build details" };
    readonly FlowLayoutPanel actions = new FlowLayoutPanel { AutoSize = true };
    readonly Button rebuild = new Button { Text = "Build again", AutoSize = true, Enabled = false };
    readonly Button files = new Button { Text = "Show files", AutoSize = true, Enabled = false };
    string project;
    Process watcher;
    bool busy;
    static string Cli { get { return Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "bin", "texe.exe"); } }

    [STAThread]
    static int Main(string[] args)
    {
        if (args.Contains("--smoke-test"))
        {
            var scratch = Path.Combine(Path.GetTempPath(), "texe-smoke-" + Guid.NewGuid());
            var paper = Path.Combine(scratch, "Å paper ' $ ` &");
            Directory.CreateDirectory(scratch);
            try
            {
                foreach (var command in new [] {
                    new [] { "--version" },
                    new [] { "init", paper, "--yes", "--title", "Å paper", "--author", "A Researcher" },
                    new [] { "editor", "--inspect", "--project", paper },
                    new [] { "adopt", paper, "--check", "--no-editor", "--yes" }
                })
                {
                    using (var process = Process.Start(Command(command)))
                    {
                        process.StandardInput.Close();
                        var stdout = process.StandardOutput.ReadToEndAsync();
                        var stderr = process.StandardError.ReadToEndAsync();
                        process.WaitForExit();
                        Task.WaitAll(stdout, stderr);
                        if (process.ExitCode != 0) return process.ExitCode;
                    }
                }
                return File.Exists(Path.Combine(paper, "main.tex")) ? 0 : 1;
            }
            finally { Directory.Delete(scratch, true); }
        }
        if (args.Length >= 2 && args[0] == "--write-args")
        {
            File.WriteAllLines(args[1], args.Skip(2).Select(value => Convert.ToBase64String(Encoding.UTF8.GetBytes(value))));
            return 0;
        }
        if (args.Contains("--test-quoting"))
        {
            if (Quote("") != "\"\"" || Quote("a b") != "\"a b\"" ||
                Quote("a\"b") != "\"a\\\"b\"" || Quote("C:\\paper\\") != "\"C:\\paper\\\\\"") return 1;
            // Exercise Windows' actual argv parser, not just expected string literals.
            if (Environment.OSVersion.Platform == PlatformID.Win32NT)
            {
                var output = Path.GetTempFileName();
                try
                {
                    var values = new [] { "", "Å paper", "a\"b", "C:\\paper\\", "$(echo bad) & `echo bad`", "--yes" };
                    var arguments = new [] { "--write-args", output }.Concat(values);
                    using (var child = Process.Start(new ProcessStartInfo(Application.ExecutablePath,
                        string.Join(" ", arguments.Select(Quote))) { UseShellExecute = false, CreateNoWindow = true }))
                    {
                        child.WaitForExit();
                        if (child.ExitCode != 0) return 1;
                    }
                    if (!File.ReadAllLines(output).SequenceEqual(values.Select(value => Convert.ToBase64String(Encoding.UTF8.GetBytes(value))))) return 1;
                }
                finally { File.Delete(output); }
            }
            return 0;
        }
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        var welcome = new Welcome();
        if (args.Length == 2 && args[0] == "--screenshot")
        {
            welcome.Shown += (s, e) => {
                var timer = new System.Windows.Forms.Timer { Interval = 1000 };
                timer.Tick += (sender, tick) => {
                    timer.Stop();
                    timer.Dispose();
                    try
                    {
                        using (var bitmap = new Bitmap(welcome.Width, welcome.Height))
                        {
                            welcome.DrawToBitmap(bitmap, new Rectangle(Point.Empty, welcome.Size));
                            bitmap.Save(args[1], System.Drawing.Imaging.ImageFormat.Png);
                        }
                        welcome.Close();
                    }
                    catch { Environment.Exit(1); }
                };
                timer.Start();
            };
        }
        Application.Run(welcome);
        return 0;
    }

    Welcome()
    {
        Text = "texe";
        ClientSize = new Size(760, 650);
        MinimumSize = new Size(720, 620);
        StartPosition = FormStartPosition.CenterScreen;
        AutoScaleMode = AutoScaleMode.Dpi;
        Font = new Font("Segoe UI", 10);
        var layout = new TableLayoutPanel { Dock = DockStyle.Fill, Padding = new Padding(24), ColumnCount = 1, RowCount = 9 };
        for (int i = 0; i < 8; i++) layout.RowStyles.Add(new RowStyle(SizeType.AutoSize));
        layout.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
        Controls.Add(layout);
        layout.Controls.Add(new Label { Text = "Your next paper starts here.", AutoSize = true,
            Font = new Font("Segoe UI", 22, FontStyle.Bold), Margin = new Padding(0, 0, 0, 12) });
        layout.Controls.Add(new Label { Text = "Write locally. texe installs the LaTeX tools and packages your paper needs.\nThe first build needs internet and may take several minutes.",
            AutoSize = true, Margin = new Padding(0, 0, 0, 16) });
        layout.Controls.Add(Row("Folder name", folderName));
        layout.Controls.Add(Row("Paper title", paperTitle));
        layout.Controls.Add(Row("Author", author));
        editor.Items.AddRange(new object[] { "VS Code", "My own editor + browser preview" });
        editor.SelectedIndex = 0;
        engine.Items.AddRange(new object[] { "pdfLaTeX", "LuaLaTeX" });
        engine.SelectedIndex = 0;
        var choices = Row("Editor", editor);
        choices.Controls.Add(engine);
        engine.AccessibleName = "LaTeX engine for new or unconfigured projects";
        layout.Controls.Add(choices);
        var create = new Button { Text = "Create a paper…", AutoSize = true };
        var open = new Button { Text = "Open a paper…", AutoSize = true };
        create.Click += async (s, e) => await CreatePaper();
        open.Click += async (s, e) => await OpenPaper();
        rebuild.Click += async (s, e) => { if (project != null) await Start(project, () => Task.FromResult(0)); };
        files.Click += (s, e) => ShowFiles();
        actions.Controls.AddRange(new Control[] { create, open, rebuild, files });
        layout.Controls.Add(actions);
        var state = new FlowLayoutPanel { AutoSize = true, Margin = new Padding(0, 12, 0, 12) };
        state.Controls.AddRange(new Control[] { progress, status });
        layout.Controls.Add(state);
        layout.Controls.Add(log);
        FormClosing += (s, e) => {
            if (busy) { e.Cancel = true; MessageBox.Show(this, "Please wait for the current setup or build to finish before quitting.", "texe"); }
            else StopWatcher();
        };
    }

    static FlowLayoutPanel Row(string label, Control control)
    {
        var row = new FlowLayoutPanel { AutoSize = true, WrapContents = false };
        row.Controls.Add(new Label { Text = label, Width = 105, Padding = new Padding(0, 5, 0, 0) });
        control.AccessibleName = label;
        row.Controls.Add(control);
        return row;
    }

    // Windows argv escaping, including embedded quotes and trailing backslashes.
    // Arguments are never interpolated into a shell command.
    static string Quote(string value)
    {
        var result = new StringBuilder("\"");
        int slashes = 0;
        foreach (char c in value)
        {
            if (c == '\\') { slashes++; continue; }
            result.Append('\\', c == '"' ? slashes * 2 + 1 : slashes);
            result.Append(c);
            slashes = 0;
        }
        result.Append('\\', slashes * 2);
        return result.Append('"').ToString();
    }

    static ProcessStartInfo Command(params string[] args)
    {
        var start = new ProcessStartInfo(Cli, string.Join(" ", args.Select(Quote))) {
            UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardOutput = true, RedirectStandardError = true, RedirectStandardInput = true,
            StandardOutputEncoding = Encoding.UTF8, StandardErrorEncoding = Encoding.UTF8,
            WorkingDirectory = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile)
        };
        start.EnvironmentVariables["PATH"] = string.Join(";", new [] {
            Path.GetDirectoryName(Cli),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "Microsoft VS Code", "bin"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Microsoft VS Code", "bin"),
            Environment.GetEnvironmentVariable("PATH") ?? ""
        });
        start.EnvironmentVariables["NO_COLOR"] = "1";
        return start;
    }

    void Append(string text)
    {
        if (IsDisposed || Disposing || !IsHandleCreated) return;
        try
        {
            BeginInvoke((Action)(() => {
                if (IsDisposed || Disposing) return;
                if (log.TextLength > 200000) log.Text = log.Text.Substring(log.TextLength - 100000);
                log.AppendText(text + Environment.NewLine);
            }));
        }
        catch (InvalidOperationException) { /* Window closed before delivery. */ }
    }

    Task Run(params string[] args)
    {
        return Task.Run(() => {
            using (var process = new Process { StartInfo = Command(args) })
            {
                process.OutputDataReceived += (s, e) => { if (e.Data != null) Append(e.Data); };
                process.ErrorDataReceived += (s, e) => { if (e.Data != null) Append(e.Data); };
                process.Start();
                process.StandardInput.Close();
                process.BeginOutputReadLine();
                process.BeginErrorReadLine();
                process.WaitForExit();
                if (process.ExitCode != 0) throw new IOException("This step could not finish. See the details, fix the issue, then try again. Your source files are kept.");
            }
        });
    }

    string PickFolder(string description)
    {
        using (var dialog = new FolderBrowserDialog { Description = description, ShowNewFolderButton = true })
            return dialog.ShowDialog(this) == DialogResult.OK ? dialog.SelectedPath : null;
    }

    async Task CreatePaper()
    {
        var parent = PickFolder("Choose where to create your paper folder.");
        if (parent == null) return;
        var name = folderName.Text.Trim();
        if (name.Length == 0 || name == "." || name == ".." || name.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0 || name.EndsWith("."))
        { MessageBox.Show(this, "Choose a simple, valid folder name.", "texe"); return; }
        var root = Path.Combine(parent, name);
        if (Directory.Exists(root) || File.Exists(root))
        { MessageBox.Show(this, "That folder already exists. Choose another name, or use Open a paper.", "texe"); return; }
        var args = new [] { "init", root, "--yes", "--quiet", "--template", "basic", "--engine", SelectedEngine,
            "--title", paperTitle.Text, "--author", author.Text };
        await Start(root, () => Run(args));
    }

    string SelectedEngine { get { return engine.SelectedIndex == 1 ? "lualatex" : "pdflatex"; } }

    async Task OpenPaper()
    {
        var root = PickFolder("Choose the folder containing your paper.");
        if (root == null) return;
        var args = new System.Collections.Generic.List<string> { "adopt", root, "--yes", "--no-build", "--no-editor" };
        if (!File.Exists(Path.Combine(root, "texe.toml")))
        {
            using (var dialog = new OpenFileDialog { Title = "Choose the main .tex file", InitialDirectory = root, Filter = "LaTeX source|*.tex" })
            {
                if (dialog.ShowDialog(this) != DialogResult.OK) return;
                var prefix = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
                if (!Path.GetFullPath(dialog.FileName).StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
                { MessageBox.Show(this, "Choose a source file inside the selected paper folder.", "texe"); return; }
                args.AddRange(new [] { "--entry", dialog.FileName.Substring(prefix.Length), "--engine", SelectedEngine });
            }
        }
        await Start(root, () => Run(args.ToArray()));
    }

    async Task Start(string root, Func<Task> prepare)
    {
        StopWatcher();
        project = root;
        busy = true;
        actions.Enabled = false;
        editor.Enabled = engine.Enabled = false;
        progress.Visible = true;
        status.Text = "Setting up and building your paper…";
        log.Clear();
        bool useCode = editor.SelectedIndex == 0;
        try
        {
            await prepare();
            await Run("build", "--project", root, "--yes");
            if (useCode)
            {
                if (!Command().EnvironmentVariables["PATH"].Split(';').Any(path => File.Exists(Path.Combine(path.Trim('"'), "code.cmd"))))
                    throw new IOException("Your PDF is ready. Install VS Code, or choose My own editor and Build again.");
                await Run("editor", "--project", root);
            }
            status.Text = useCode ? "PDF ready. Editor setup details are below." : "Paper built. You can start writing.";
            if (!useCode) { StartWatcher(root); ShowFiles(); }
        }
        catch (Exception error) { Append(error.Message); status.Text = "Needs attention — see the details below."; }
        finally
        {
            busy = false;
            actions.Enabled = true;
            rebuild.Enabled = File.Exists(Path.Combine(root, "texe.toml"));
            files.Enabled = Directory.Exists(root);
            editor.Enabled = engine.Enabled = true;
            progress.Visible = false;
        }
    }

    void StartWatcher(string root)
    {
        var process = new Process { StartInfo = Command("watch", "--view", "--project", root, "--yes"), EnableRaisingEvents = true };
        process.OutputDataReceived += (s, e) => { if (e.Data != null) Append(e.Data); };
        process.ErrorDataReceived += (s, e) => { if (e.Data != null) Append(e.Data); };
        process.Exited += (s, e) => {
            if (!IsDisposed && IsHandleCreated) BeginInvoke((Action)(() => {
                if (watcher == process) { watcher = null; status.Text = "Live preview stopped. Choose Build again to restart."; process.Dispose(); }
            }));
        };
        process.Start();
        process.StandardInput.Close();
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();
        watcher = process;
        status.Text = "Watching saves. Keep texe open for live preview.";
    }

    void StopWatcher()
    {
        var process = watcher;
        watcher = null;
        if (process == null) return;
        try
        {
            if (!process.HasExited)
            {
                // Stop only this watcher's process tree, including an active build.
                using (var kill = Process.Start(new ProcessStartInfo(
                    Path.Combine(Environment.SystemDirectory, "taskkill.exe"), "/PID " + process.Id + " /T /F")
                    { UseShellExecute = false, CreateNoWindow = true })) kill.WaitForExit();
                process.WaitForExit();
            }
        }
        finally { process.Dispose(); }
    }

    void ShowFiles()
    {
        if (project != null) Process.Start(new ProcessStartInfo(project) { UseShellExecute = true });
    }
}
