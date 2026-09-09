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
        if (args.Length == 2 && (args[0] == "--screenshot" || args[0] == "--screenshot-setup"))
        {
            welcome.Shown += (s, e) => {
                if (args[0] == "--screenshot-setup") welcome.ShowPage(welcome.setup);
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

    readonly Color ink = Color.FromArgb(48, 43, 47);
    readonly Color muted = Color.FromArgb(121, 114, 119);
    readonly Color accent = Color.FromArgb(93, 70, 84);
    readonly Panel workspace = new Panel { Width = 580, Height = 540 };
    readonly Panel home = new Panel { Dock = DockStyle.Fill };
    readonly Panel setup = new Panel { Dock = DockStyle.Fill, Visible = false };
    readonly Panel activity = new Panel { Dock = DockStyle.Fill, Visible = false };
    readonly Panel preferences = new Panel { Width = 532, Height = 102, Visible = false };
    readonly Button back = new QuietButton { Text = "←  Your papers" };
    readonly Button details = new QuietButton { Text = "Show details" };

    Welcome()
    {
        Text = "texe";
        AutoScaleDimensions = new SizeF(96, 96);
        AutoScaleMode = AutoScaleMode.Dpi;
        ClientSize = new Size(940, 680);
        MinimumSize = new Size(900, 700);
        StartPosition = FormStartPosition.CenterScreen;
        Font = new Font("Segoe UI", 10);
        BackColor = Color.FromArgb(253, 252, 250);
        ForeColor = ink;
        DoubleBuffered = true;
        var assembly = typeof(Welcome).Assembly;
        using (var stream = assembly.GetManifestResourceStream("texe.icon"))
            if (stream != null) Icon = new Icon(stream);
        var sidebar = new Panel { Dock = DockStyle.Left, Width = 208, BackColor = Color.FromArgb(244, 241, 238) };
        Controls.Add(sidebar);
        using (var stream = assembly.GetManifestResourceStream("texe.wordmark"))
            if (stream != null) sidebar.Controls.Add(new PictureBox { Image = new Bitmap(stream),
                SizeMode = PictureBoxSizeMode.Zoom, Bounds = new Rectangle(18, 34, 137, 48) });
        var nav = Button("Your papers", false);
        nav.Bounds = new Rectangle(18, 120, 172, 42);
        nav.BackColor = Color.FromArgb(231, 225, 229);
        nav.Click += (s, e) => { if (!busy) ShowPage(home); };
        sidebar.Controls.Add(nav);
        var foot = Label("A little less setup.\nA little more writing.", 10, false, muted);
        foot.SetBounds(28, 564, 160, 55);
        foot.Anchor = AnchorStyles.Left | AnchorStyles.Bottom;
        sidebar.Controls.Add(foot);
        var body = new Panel { Dock = DockStyle.Fill };
        Controls.Add(body);
        body.BringToFront();
        body.Controls.Add(workspace);
        body.Resize += (s, e) => workspace.Location = new Point(Math.Max(24, (body.Width - workspace.Width) / 2), Math.Max(28, (body.Height - workspace.Height) / 2));
        workspace.Controls.AddRange(new Control[] { home, setup, activity });
        Shown += (s, e) => workspace.Location = new Point(Math.Max(24, (body.Width - workspace.Width) / 2), Math.Max(28, (body.Height - workspace.Height) / 2));

        AddText(home, "YOUR WORKSPACE", 10, true, muted, 0, 14, 540, 24);
        AddText(home, "Space for your next idea.", 29, true, ink, 0, 60, 570, 52);
        AddText(home, "Start a paper. Make it yours.", 12, false, muted, 0, 121, 540, 32);
        var illustration = new PaperIllustration { Bounds = new Rectangle(0, 181, 532, 156) };
        home.Controls.Add(illustration);
        var create = Button("+    New paper", true);
        create.Bounds = new Rectangle(0, 365, 258, 50);
        create.Click += (s, e) => ShowPage(setup);
        var open = Button("Open a paper…", false);
        open.Bounds = new Rectangle(274, 365, 258, 50);
        open.Click += async (s, e) => await OpenPaper();
        home.Controls.AddRange(new Control[] { create, open });
        Shown += (s, e) => ActiveControl = create;
        AddText(home, "Your files stay on your computer.\ntexe takes care of the tools your paper needs.", 10, false, muted, 0, 443, 530, 58);

        back.Bounds = new Rectangle(0, 0, 160, 32);
        back.Click += (s, e) => ShowPage(home);
        setup.Controls.Add(back);
        AddText(setup, "Make room for a new paper.", 25, true, ink, 0, 52, 560, 48);
        AddText(setup, "A title is a good place to start. You can change it later.", 10, false, muted, 0, 106, 560, 28);
        Field(setup, "Paper title", paperTitle, 156, 532);
        Field(setup, "Author", author, 241, 532);
        author.Text = "";
        editor.Items.AddRange(new object[] { "VS Code", "My own editor + browser preview" });
        editor.SelectedIndex = 0;
        engine.Items.AddRange(new object[] { "pdfLaTeX", "LuaLaTeX" });
        engine.SelectedIndex = 0;
        engine.AccessibleName = "LaTeX engine";
        var advanced = new QuietButton { Text = "Writing preferences  ⌄", Bounds = new Rectangle(0, 321, 230, 32) };
        advanced.Click += (s, e) => { preferences.Visible = !preferences.Visible; advanced.Text = preferences.Visible ? "Writing preferences  ⌃" : "Writing preferences  ⌄"; };
        setup.Controls.Add(advanced);
        preferences.Location = new Point(0, 365);
        Field(preferences, "Editor", editor, 0, 330);
        Field(preferences, "Typesetting", engine, 0, 180, 352);
        setup.Controls.Add(preferences);
        var submit = Button("Choose location & create", true);
        submit.Bounds = new Rectangle(0, 479, 280, 48);
        submit.Click += async (s, e) => await CreatePaper();
        setup.Controls.Add(submit);

        AddText(activity, "YOUR PAPER", 10, true, muted, 0, 14, 530, 24);
        AddText(activity, "A little preparation.\nThen it’s all yours.", 28, true, ink, 0, 60, 540, 108);
        status.AutoSize = false;
        status.Font = new Font("Segoe UI", 12);
        status.SetBounds(0, 199, 532, 62);
        activity.Controls.Add(status);
        progress.SetBounds(0, 278, 532, 4);
        activity.Controls.Add(progress);
        AddText(activity, "The first setup can take a few minutes.\nYou can leave this window open while we get things ready.", 10, false, muted, 0, 302, 532, 52);
        actions.SetBounds(0, 377, 540, 48);
        actions.AutoSize = false;
        foreach (var button in new [] { rebuild, files }) {
            button.AutoSize = false; button.Size = new Size(150, 42);
            button.FlatStyle = FlatStyle.Flat; button.FlatAppearance.BorderColor = Color.FromArgb(223, 216, 221);
            button.BackColor = Color.White; button.Margin = new Padding(0, 0, 12, 0);
        }
        actions.Controls.AddRange(new Control[] { rebuild, files });
        rebuild.Click += async (s, e) => { if (project != null) await Start(project, () => Task.FromResult(0)); };
        files.Click += (s, e) => ShowFiles();
        activity.Controls.Add(actions);
        details.SetBounds(0, 443, 140, 32);
        details.Click += (s, e) => {
            using (var dialog = new Form { Text = "Build details · texe", Size = new Size(780, 460), StartPosition = FormStartPosition.CenterParent }) {
                log.BorderStyle = BorderStyle.None; dialog.Padding = new Padding(20); dialog.Controls.Add(log);
                dialog.ShowDialog(this); dialog.Controls.Remove(log);
            }
        };
        activity.Controls.Add(details);
        FormClosing += (s, e) => {
            if (busy) { e.Cancel = true; MessageBox.Show(this, "Please wait for the current setup or build to finish before quitting.", "texe"); }
            else StopWatcher();
        };
    }

    void ShowPage(Panel page)
    {
        home.Visible = setup.Visible = activity.Visible = false;
        page.Visible = true;
        page.BringToFront();
        if (page == setup) paperTitle.Focus();
    }

    Label Label(string text, float size, bool bold, Color color)
    {
        return new Label { Text = text, Font = new Font("Segoe UI", size, bold ? FontStyle.Bold : FontStyle.Regular), ForeColor = color };
    }

    void AddText(Control parent, string text, float size, bool bold, Color color, int x, int y, int w, int h)
    {
        var label = Label(text, size, bold, color); label.SetBounds(x, y, w, h); parent.Controls.Add(label);
    }

    Button Button(string text, bool primary)
    {
        return new QuietButton { Text = text, BackColor = primary ? accent : Color.White,
            ForeColor = primary ? Color.White : ink, Font = new Font("Segoe UI", 11, FontStyle.Bold) };
    }

    void Field(Control parent, string label, Control field, int y, int width, int x = 0)
    {
        AddText(parent, label, 10, true, muted, x, y, width, 25);
        field.SetBounds(x, y + 30, width, 32);
        field.Font = new Font("Segoe UI", 12);
        field.AccessibleName = label;
        var textbox = field as TextBox;
        if (textbox != null) {
            var surround = new Panel { Bounds = new Rectangle(x, y + 30, width, 44), BackColor = Color.White };
            surround.Paint += (s, e) => { using (var pen = new Pen(textbox.Focused ? accent : Color.FromArgb(222, 215, 220))) e.Graphics.DrawRectangle(pen, 0, 0, surround.Width - 1, surround.Height - 1); };
            textbox.BorderStyle = BorderStyle.None;
            textbox.BackColor = Color.White;
            textbox.SetBounds(12, 10, width - 24, 26);
            textbox.GotFocus += (s, e) => surround.Invalidate();
            textbox.LostFocus += (s, e) => surround.Invalidate();
            surround.Controls.Add(textbox);
            parent.Controls.Add(surround);
            return;
        }
        var combo = field as ComboBox;
        if (combo != null) combo.FlatStyle = FlatStyle.Flat;
        parent.Controls.Add(field);
    }

    sealed class QuietButton : Button
    {
        public QuietButton() { DoubleBuffered = true; FlatStyle = FlatStyle.Flat; FlatAppearance.BorderSize = 0; Cursor = Cursors.Hand; }
        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.Clear(Parent == null ? SystemColors.Control : Parent.BackColor);
            e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
            var r = new Rectangle(1, 1, Width - 3, Height - 3);
            using (var path = new System.Drawing.Drawing2D.GraphicsPath()) {
                const int d = 14;
                path.AddArc(r.Left, r.Top, d, d, 180, 90); path.AddArc(r.Right - d, r.Top, d, d, 270, 90);
                path.AddArc(r.Right - d, r.Bottom - d, d, d, 0, 90); path.AddArc(r.Left, r.Bottom - d, d, d, 90, 90); path.CloseFigure();
                using (var brush = new SolidBrush(Enabled ? BackColor : SystemColors.Control)) e.Graphics.FillPath(brush, path);
                using (var pen = new Pen(Focused ? Color.FromArgb(93, 70, 84) : Color.FromArgb(226, 220, 224), Focused ? 2 : 1)) e.Graphics.DrawPath(pen, path);
            }
            TextRenderer.DrawText(e.Graphics, Text, Font, r, Enabled ? ForeColor : SystemColors.GrayText, TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter | TextFormatFlags.NoPrefix);
        }
    }

    sealed class PaperIllustration : Control
    {
        public PaperIllustration() { DoubleBuffered = true; }
        protected override void OnPaint(PaintEventArgs e)
        {
            var g = e.Graphics; g.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
            g.Clear(Color.FromArgb(244, 240, 237));
            using (var shadow = new SolidBrush(Color.FromArgb(229, 220, 224))) g.FillRectangle(shadow, 38, 28, 100, 128);
            g.FillRectangle(Brushes.White, 30, 20, 100, 130);
            using (var pen = new Pen(Color.FromArgb(93, 70, 84), 3)) g.DrawLine(pen, 47, 45, 94, 45);
            using (var pen = new Pen(Color.FromArgb(222, 215, 218), 2))
                for (int i = 0; i < 5; i++) g.DrawLine(pen, 47, 65 + i * 12, i == 4 ? 90 : 111, 65 + i * 12);
            using (var title = new Font("Segoe UI", 13, FontStyle.Bold))
                TextRenderer.DrawText(g, "Good ideas start here.", title, new Point(164, 43), Color.FromArgb(65, 52, 61));
            using (var font = new Font("Segoe UI", 10))
                TextRenderer.DrawText(g, "A clean page, ready for your words.\nBeautifully typeset from the first draft.", font, new Rectangle(164, 77, 348, 64), Color.FromArgb(121, 114, 119), TextFormatFlags.Left | TextFormatFlags.Top | TextFormatFlags.NoPadding);
        }
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
        var name = paperTitle.Text.Trim();
        foreach (char c in Path.GetInvalidFileNameChars()) name = name.Replace(c, '-');
        name = name.TrimEnd('.');
        if (name.Length == 0) name = "Untitled paper";
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
        ShowPage(activity);
        StopWatcher();
        project = root;
        busy = true;
        actions.Enabled = false;
        editor.Enabled = engine.Enabled = false;
        progress.Visible = true;
        status.Text = "Getting your paper ready…";
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
            status.Text = useCode ? "Your PDF is ready. Open Show details for editor setup." : "Paper built. You can start writing.";
            if (!useCode) { StartWatcher(root); ShowFiles(); }
        }
        catch (Exception error) { Append(error.Message); status.Text = "Something needs your attention. Open Show details for help."; }
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
