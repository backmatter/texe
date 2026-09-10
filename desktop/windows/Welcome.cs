using System;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Net;
using System.Security.Cryptography;
using System.Text.RegularExpressions;
using System.Runtime.InteropServices;
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
    readonly TextBox status = new TextBox { ReadOnly = true, Multiline = true, BorderStyle = BorderStyle.None, TabStop = false };
    readonly ProgressBar progress = new ProgressBar { Style = ProgressBarStyle.Marquee, Visible = false, Width = 110 };
    readonly TextBox log = new TextBox { Multiline = true, ReadOnly = true, ScrollBars = ScrollBars.Vertical,
        Dock = DockStyle.Fill, Font = new Font("Consolas", 9), AccessibleName = "Setup and build details" };
    readonly FlowLayoutPanel actions = new FlowLayoutPanel { AutoSize = true };
    readonly Button rebuild = new Button { Text = "Build again", AutoSize = true, Enabled = false };
    readonly Button openCode = new QuietButton { Text = "Open in VS Code", Enabled = false };
    readonly Button nextPaper = new QuietButton { Text = "New paper" };
    readonly TextBox activityTitle = Label("", 28, true, Color.FromArgb(48, 43, 47));
    readonly TextBox activityHint = Label("", 10, false, Color.FromArgb(121, 114, 119));
    readonly Button files = new Button { Text = "Show files", AutoSize = true, Enabled = false };
    string project;
    string parentFolder = Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments);
    readonly TextBox destination = new TextBox { ReadOnly = true, BorderStyle = BorderStyle.None, TabStop = false };
    readonly ToolTip locationTip = new ToolTip();
    Process watcher;
    bool busy;
    static string Cli { get { return Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "bin", "texe.exe"); } }

    [STAThread]
    static int Main(string[] args)
    {
        if (args.Length == 2 && args[0] == "--hold-output")
        {
            File.WriteAllText(args[1], Process.GetCurrentProcess().Id.ToString());
            System.Threading.Thread.Sleep(20000);
            return 0;
        }
        if (args.Length == 2 && args[0] == "--spawn-output-holder")
        {
            using (var child = Process.Start(new ProcessStartInfo(Application.ExecutablePath,
                "--hold-output " + Quote(args[1])) { UseShellExecute = false,
                    CreateNoWindow = true, RedirectStandardInput = true }))
            { child.StandardInput.Close(); }
            Console.WriteLine("setup completed");
            return 7;
        }
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
                ExplorerFolderPicker.Verify();
                VerifyProcessCompletion();
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
        if (args.Length == 2 && (args[0] == "--screenshot" || args[0] == "--screenshot-setup" || args[0] == "--screenshot-code"))
        {
            welcome.Shown += (s, e) => {
                if (args[0] == "--screenshot-setup") welcome.ShowPage(welcome.setup);
                if (args[0] == "--screenshot-code") welcome.ShowPage(welcome.codeSetup);
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
    readonly Panel workspace = new Panel { Width = 532, Height = 504 };
    readonly Panel codeSetup = new Panel { Dock = DockStyle.Fill, Visible = false };
    readonly TextBox codeStatus = new TextBox { ReadOnly = true, Multiline = true, BorderStyle = BorderStyle.None, TabStop = false };
    readonly Button installCode = new QuietButton { Text = "Install VS Code" };
    readonly Button checkCode = new QuietButton { Text = "I’ve installed it" };
    Func<Task> pendingSetup;
    readonly Panel home = new Panel { Dock = DockStyle.Fill };
    readonly Panel setup = new Panel { Dock = DockStyle.Fill, Visible = false };
    readonly Panel activity = new Panel { Dock = DockStyle.Fill, Visible = false };
    readonly Button back = new QuietButton { Text = "←  Your papers" };
    readonly Button details = new QuietButton { Text = "Show details" };

    Welcome()
    {
        Text = "texe";
        AutoScaleDimensions = new SizeF(96, 96);
        AutoScaleMode = AutoScaleMode.Dpi;
        ClientSize = new Size(860, 600);
        FormBorderStyle = FormBorderStyle.FixedSingle;
        MaximizeBox = false;
        StartPosition = FormStartPosition.CenterScreen;
        Font = new Font("Segoe UI", 10);
        BackColor = Color.FromArgb(253, 252, 250);
        ForeColor = ink;
        DoubleBuffered = true;
        var assembly = typeof(Welcome).Assembly;
        using (var stream = assembly.GetManifestResourceStream("texe.icon"))
            if (stream != null) Icon = new Icon(stream);
        var sidebar = new Panel { Dock = DockStyle.Left, Width = 184, BackColor = Color.FromArgb(244, 241, 238) };
        Controls.Add(sidebar);
        using (var stream = assembly.GetManifestResourceStream("texe.wordmark"))
            if (stream != null) sidebar.Controls.Add(new PictureBox { Image = new Bitmap(stream),
                SizeMode = PictureBoxSizeMode.Zoom, Bounds = new Rectangle(18, 28, 137, 44) });
        var nav = Button("Your papers", false);
        nav.Bounds = new Rectangle(18, 100, 148, 40);
        nav.BackColor = Color.FromArgb(231, 225, 229);
        nav.Click += (s, e) => { if (!busy) ShowPage(home); };
        sidebar.Controls.Add(nav);
        var foot = Label("A little less setup.\nA little more writing.", 10, false, muted);
        foot.SetBounds(24, 510, 152, 50);
        foot.BackColor = sidebar.BackColor;
        foot.Anchor = AnchorStyles.Left | AnchorStyles.Bottom;
        sidebar.Controls.Add(foot);
        var body = new Panel { Dock = DockStyle.Fill };
        Controls.Add(body);
        body.BringToFront();
        body.Controls.Add(workspace);
        body.Resize += (s, e) => workspace.Location = new Point(Math.Max(24, (body.Width - workspace.Width) / 2), Math.Max(28, (body.Height - workspace.Height) / 2));
        workspace.Controls.AddRange(new Control[] { home, setup, activity, codeSetup });
        CreateCodeSetup();
        Shown += (s, e) => workspace.Location = new Point(Math.Max(24, (body.Width - workspace.Width) / 2), Math.Max(28, (body.Height - workspace.Height) / 2));

        AddText(home, "YOUR WORKSPACE", 10, true, muted, 0, 0, 532, 24);
        AddText(home, "Space for your next idea.", 26, true, ink, 0, 36, 532, 45);
        AddText(home, "Start a paper. Make it yours.", 12, false, muted, 0, 92, 532, 26);
        var illustration = new Panel { Bounds = new Rectangle(0, 148, 532, 144), BackColor = Color.FromArgb(244, 240, 237) };
        illustration.Controls.Add(new PaperIllustration { Bounds = new Rectangle(0, 0, 144, 144) });
        AddText(illustration, "Good ideas start here.", 13, true, ink, 164, 36, 348, 28);
        AddText(illustration, "A clean page, ready for your words.\nBeautifully typeset from the first draft.", 10, false, muted, 164, 74, 348, 48);
        home.Controls.Add(illustration);
        var create = Button("+    New paper", true);
        create.Bounds = new Rectangle(0, 316, 258, 44);
        create.Click += (s, e) => ShowPage(setup);
        var open = Button("Open a paper…", false);
        open.Bounds = new Rectangle(274, 316, 258, 44);
        open.Click += async (s, e) => await OpenPaper();
        home.Controls.AddRange(new Control[] { create, open });
        Shown += (s, e) => ActiveControl = create;
        AddText(home, "Your files stay on your computer.\ntexe takes care of the tools your paper needs.", 10, false, muted, 0, 392, 532, 48);

        back.Bounds = new Rectangle(0, 0, 160, 32);
        back.Click += (s, e) => ShowPage(home);
        setup.Controls.Add(back);
        AddText(setup, "Make room for a new paper.", 22, true, ink, 0, 48, 532, 40);
        AddText(setup, "A title is a good place to start. You can change it later.", 10, false, muted, 0, 94, 532, 26);
        Field(setup, "Paper title", paperTitle, 140, 532);
        Field(setup, "Author", author, 224, 532);
        author.Text = "";
        editor.Items.AddRange(new object[] { "VS Code", "My own editor + browser preview" });
        editor.SelectedIndex = 0;
        engine.Items.AddRange(new object[] { "pdfLaTeX", "LuaLaTeX" });
        engine.SelectedIndex = 0;
        engine.AccessibleName = "LaTeX engine";
        Field(setup, "Editor", editor, 308, 330);
        Field(setup, "Typesetting", engine, 308, 186, 346);
        AddText(setup, "Project folder", 10, true, muted, 0, 390, 420, 22);
        destination.SetBounds(0, 420, 416, 25);
        destination.BackColor = BackColor;
        destination.AccessibleName = "Final project folder";
        setup.Controls.Add(destination);
        var browse = Button("Change…", false);
        browse.SetBounds(432, 410, 100, 36);
        browse.Click += (s, e) => { var folder = PickFolder("Choose a parent folder. Your paper gets its own subfolder."); if (folder != null) { parentFolder = folder; UpdateDestination(); } };
        setup.Controls.Add(browse);
        paperTitle.TextChanged += (s, e) => UpdateDestination();
        UpdateDestination();
        var submit = Button("Create paper", true);
        submit.Bounds = new Rectangle(0, 460, 220, 44);
        submit.Click += async (s, e) => await CreatePaper();
        setup.Controls.Add(submit);

        AddText(activity, "YOUR PAPER", 10, true, muted, 0, 14, 530, 24);
        activityTitle.BackColor = BackColor;
        activityHint.BackColor = BackColor;
        activityTitle.SetBounds(0, 60, 532, 108);
        activity.Controls.Add(activityTitle);
        status.AutoSize = false;
        status.BackColor = BackColor;
        status.Font = new Font("Segoe UI", 12);
        status.SetBounds(0, 199, 532, 62);
        activity.Controls.Add(status);
        progress.SetBounds(0, 278, 532, 4);
        activity.Controls.Add(progress);
        activityHint.SetBounds(0, 302, 532, 52);
        activity.Controls.Add(activityHint);
        actions.SetBounds(0, 377, 540, 48);
        actions.AutoSize = false;
        foreach (var button in new [] { nextPaper, openCode, files }) {
            button.AutoSize = false; button.Size = new Size(150, 42);
            button.FlatStyle = FlatStyle.Flat; button.FlatAppearance.BorderColor = Color.FromArgb(223, 216, 221);
            button.BackColor = Color.White; button.Margin = new Padding(0, 0, 12, 0);
        }
        actions.Controls.AddRange(new Control[] { nextPaper, openCode, files });
        nextPaper.Click += (s, e) => { paperTitle.Text = "Untitled paper"; ShowPage(setup); paperTitle.Focus(); paperTitle.SelectAll(); };
        openCode.Click += async (s, e) => {
            if (project == null || busy) return;
            openCode.Enabled = false;
            try { await Run("editor", "--project", project); }
            catch (Exception error) { status.Text = error.Message; }
            finally { openCode.Enabled = true; }
        };
        rebuild.SetBounds(152, 443, 140, 32);
        activity.Controls.Add(rebuild);
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

    bool CodeAvailable
    {
        get { return Command().EnvironmentVariables["PATH"].Split(';').Any(path => File.Exists(Path.Combine(path.Trim('"'), "code.cmd"))); }
    }

    void CreateCodeSetup()
    {
        AddText(codeSetup, "ONE-TIME SETUP", 10, true, muted, 0, 0, 532, 24);
        AddText(codeSetup, "Set up your writing app.", 26, true, ink, 0, 42, 532, 48);
        AddText(codeSetup, "texe opens your paper in Visual Studio Code.\nInstall it once, then get straight to writing.", 12, false, muted, 0, 110, 532, 64);
        codeStatus.SetBounds(0, 208, 532, 92);
        codeStatus.BackColor = BackColor;
        codeStatus.Font = new Font("Segoe UI", 11);
        codeStatus.Text = "VS Code isn’t installed on this computer.";
        codeSetup.Controls.Add(codeStatus);
        installCode.SetBounds(0, 328, 258, 44);
        installCode.BackColor = accent;
        installCode.ForeColor = Color.White;
        installCode.Click += async (s, e) => await InstallCode();
        checkCode.SetBounds(274, 328, 258, 44);
        checkCode.BackColor = Color.White;
        checkCode.Click += async (s, e) => await ContinueFromCode();
        codeSetup.Controls.AddRange(new Control[] { installCode, checkCode });
        var backToSetup = Button("Back to paper setup", false);
        backToSetup.SetBounds(0, 410, 220, 36);
        backToSetup.Click += (s, e) => { if (!busy) ShowPage(setup); };
        codeSetup.Controls.Add(backToSetup);
    }

    async Task ContinueFromCode()
    {
        if (!CodeAvailable) {
            codeStatus.Text = "We couldn’t find VS Code yet. Finish the installer, then try again.";
            return;
        }
        var resume = pendingSetup;
        pendingSetup = null;
        if (resume != null) await resume(); else ShowPage(setup);
    }

    async Task InstallCode()
    {
        busy = true;
        installCode.Enabled = checkCode.Enabled = false;
        var scratch = Path.Combine(Path.GetTempPath(), "texe-vscode-" + Guid.NewGuid());
        bool installed = false;
        try {
            Directory.CreateDirectory(scratch);
            var installer = Path.Combine(scratch, "VSCodeUserSetup.exe");
            ServicePointManager.SecurityProtocol |= SecurityProtocolType.Tls12;
            using (var download = new WebClient()) {
                codeStatus.Text = "Finding the latest VS Code installer…";
                var metadata = await download.DownloadStringTaskAsync("https://update.code.visualstudio.com/api/update/win32-x64-user/stable/latest");
                var url = Regex.Match(metadata, "\"url\"\\s*:\\s*\"([^\"]+)\"").Groups[1].Value.Replace("\\/", "/");
                var hash = Regex.Match(metadata, "\"sha256hash\"\\s*:\\s*\"([a-fA-F0-9]{64})\"").Groups[1].Value;
                Uri address;
                if (!Uri.TryCreate(url, UriKind.Absolute, out address) || address.Scheme != "https" || hash.Length != 64)
                    throw new IOException("The installer download could not be verified. Please try again.");
                download.DownloadProgressChanged += (s, e) => codeStatus.Text = "Downloading VS Code… " + e.ProgressPercentage + "%";
                await download.DownloadFileTaskAsync(address, installer);
                codeStatus.Text = "Checking the VS Code download…";
                await Task.Run(() => {
                    using (var file = File.OpenRead(installer))
                    using (var sha = SHA256.Create()) {
                        var actual = BitConverter.ToString(sha.ComputeHash(file)).Replace("-", "");
                        if (!actual.Equals(hash, StringComparison.OrdinalIgnoreCase))
                            throw new IOException("The installer download could not be verified. Please try again.");
                    }
                });
            }
            codeStatus.Text = "Follow the VS Code installer. We’ll continue when it finishes.";
            await Task.Run(() => {
                using (var process = Process.Start(new ProcessStartInfo(installer, "/NORESTART /MERGETASKS=!runcode") { UseShellExecute = true }))
                    process.WaitForExit();
            });
            installed = CodeAvailable;
            if (!installed) codeStatus.Text = "Installation wasn’t completed. Choose Install VS Code to try again, or finish installing and choose I’ve installed it.";
        } catch (Exception error) {
            codeStatus.Text = "VS Code couldn’t be installed. " + error.Message;
            Append(error.Message);
        } finally {
            busy = false;
            installCode.Enabled = checkCode.Enabled = true;
            try { if (Directory.Exists(scratch)) Directory.Delete(scratch, true); } catch (IOException) { }
        }
        if (installed) await ContinueFromCode();
    }

    void ShowPage(Panel page)
    {
        home.Visible = setup.Visible = activity.Visible = codeSetup.Visible = false;
        page.Visible = true;
        page.BringToFront();
        if (page == setup) paperTitle.Focus();
    }

    static TextBox Label(string text, float size, bool bold, Color color)
    {
        return new TextBox { Text = text.Replace("\n", Environment.NewLine), Multiline = true, ReadOnly = true, BorderStyle = BorderStyle.None, TabStop = false, Font = new Font("Segoe UI", size, bold ? FontStyle.Bold : FontStyle.Regular), ForeColor = color };
    }

    void AddText(Control parent, string text, float size, bool bold, Color color, int x, int y, int w, int h)
    {
        var label = Label(text, size, bold, color); label.BackColor = parent.BackColor; label.SetBounds(x, y, w, h); parent.Controls.Add(label);
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
            g.ScaleTransform(Width / 144f, Height / 144f);
            using (var shadow = new SolidBrush(Color.FromArgb(229, 220, 224))) g.FillRectangle(shadow, 32, 26, 94, 110);
            g.FillRectangle(Brushes.White, 24, 20, 94, 110);
            using (var pen = new Pen(Color.FromArgb(93, 70, 84), 3)) g.DrawLine(pen, 40, 42, 87, 42);
            using (var pen = new Pen(Color.FromArgb(222, 215, 218), 2))
                for (int i = 0; i < 5; i++) g.DrawLine(pen, 40, 61 + i * 12, i == 4 ? 83 : 104, 61 + i * 12);

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

    // The CLI can exit while an editor descendant still owns an inherited pipe.
    // Parameterless WaitForExit waits for pipe EOF too, leaving setup stuck until
    // the editor closes. Wait for the CLI, then allow a bounded output drain.
    static int RunCommand(ProcessStartInfo start, Action<string> append)
    {
        using (var process = new Process { StartInfo = start })
        {
            var stdout = new TaskCompletionSource<bool>();
            var stderr = new TaskCompletionSource<bool>();
            process.OutputDataReceived += (s, e) => { if (e.Data == null) stdout.TrySetResult(true); else append(e.Data); };
            process.ErrorDataReceived += (s, e) => { if (e.Data == null) stderr.TrySetResult(true); else append(e.Data); };
            process.Start();
            process.StandardInput.Close();
            process.BeginOutputReadLine();
            process.BeginErrorReadLine();
            while (!process.WaitForExit(1000)) { }
            Task.WaitAll(new Task[] { stdout.Task, stderr.Task }, 1000);
            process.CancelOutputRead();
            process.CancelErrorRead();
            return process.ExitCode;
        }
    }

    static void VerifyProcessCompletion()
    {
        var marker = Path.GetTempFileName();
        try
        {
            var start = new ProcessStartInfo(Application.ExecutablePath,
                "--spawn-output-holder " + Quote(marker)) { UseShellExecute = false,
                    CreateNoWindow = true, RedirectStandardInput = true,
                    RedirectStandardOutput = true, RedirectStandardError = true };
            var output = new System.Collections.Concurrent.ConcurrentQueue<string>();
            var timer = Stopwatch.StartNew();
            var code = RunCommand(start, output.Enqueue);
            if (code != 7 || timer.ElapsedMilliseconds > 5000 || !output.Contains("setup completed"))
                throw new InvalidOperationException("Setup must finish when the CLI exits, preserving output and exit status.");
        }
        finally
        {
            int pid;
            if (int.TryParse(File.ReadAllText(marker), out pid))
                try { using (var holder = Process.GetProcessById(pid)) holder.Kill(); }
                catch (ArgumentException) { }
            File.Delete(marker);
        }
    }

    Task Run(params string[] args)
    {
        return Task.Run(() => {
            if (RunCommand(Command(args), Append) != 0)
                throw new IOException("This step could not finish. See the details, fix the issue, then try again. Your source files are kept.");
        });
    }

    string PickFolder(string description)
    {
        return ExplorerFolderPicker.Pick(Handle, description);
    }

    string ProjectFolderName
    {
        get {
            var name = paperTitle.Text.Trim();
            foreach (char c in Path.GetInvalidFileNameChars()) name = name.Replace(c, '-');
            name = name.TrimEnd('.');
            return name.Length == 0 ? "Untitled paper" : name;
        }
    }
    void UpdateDestination()
    {
        destination.Text = Path.Combine(parentFolder, ProjectFolderName);
        locationTip.SetToolTip(destination, destination.Text);
    }

    async Task CreatePaper()
    {
        var parent = parentFolder;
        var name = ProjectFolderName;
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
        if (editor.SelectedIndex == 0 && !CodeAvailable) {
            pendingSetup = () => Start(root, prepare);
            codeStatus.Text = "VS Code isn’t installed on this computer.";
            ShowPage(codeSetup);
            return;
        }
        ShowPage(activity);
        StopWatcher();
        project = root;
        busy = true;
        actions.Enabled = false;
        editor.Enabled = engine.Enabled = false;
        progress.Visible = true;
        activityTitle.Text = "A little preparation.\r\nThen it’s all yours.";
        activityHint.Text = "The first setup can take a few minutes.\r\nYou can leave this window open while we get things ready.";
        rebuild.Enabled = false;
        status.Text = "Getting your paper ready…";
        log.Clear();
        bool useCode = editor.SelectedIndex == 0;
        try
        {
            await prepare();
            await Run("build", "--project", root, "--yes");
            if (useCode)
            {
                await Run("editor", "--project", root);
            }
            activityTitle.Text = "Your paper is ready.";
            activityHint.Text = "Keep writing, or start something new.";
            status.Text = useCode ? "Your paper is ready in VS Code. If asked, choose Trust to enable live preview." : "Paper built. You can start writing.";
            if (!useCode) { StartWatcher(root); ShowFiles(); }
        }
        catch (Exception error) { activityTitle.Text = "Let’s finish setting up."; activityHint.Text = "Your source files are kept. You can retry or start another paper."; Append(error.Message); status.Text = error.Message; }
        finally
        {
            busy = false;
            actions.Enabled = true;
            rebuild.Enabled = File.Exists(Path.Combine(root, "texe.toml"));
            files.Enabled = Directory.Exists(root);
            openCode.Enabled = CodeAvailable && File.Exists(Path.Combine(root, "texe.toml"));
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

// Windows' Common Item Dialog in folder mode: Explorer navigation, address bar,
// search and New folder. The COM declarations follow shobjidl_core.h vtable order.
internal static class ExplorerFolderPicker
{
    const uint FolderOptions = 0x20 | 0x40 | 0x800 | 0x8;
    static IFileDialog NewDialog()
    {
        return (IFileDialog)Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7")));
    }
    public static void Verify()
    {
        var dialog = NewDialog();
        try {
            uint options; dialog.GetOptions(out options);
            dialog.SetOptions(options | FolderOptions);
            dialog.GetOptions(out options);
            if ((options & FolderOptions) != FolderOptions) throw new InvalidOperationException("Explorer folder options were not applied.");
        } finally { Marshal.FinalReleaseComObject(dialog); }
    }
    public static string Pick(IntPtr owner, string title)
    {
        var dialog = NewDialog();
        IShellItem item = null;
        IntPtr path = IntPtr.Zero;
        try {
            uint options; dialog.GetOptions(out options);
            dialog.SetOptions(options | FolderOptions);
            dialog.SetTitle(title);
            dialog.SetOkButtonLabel("Select folder");
            int result = dialog.Show(owner);
            if (result == unchecked((int)0x800704C7)) return null;
            Marshal.ThrowExceptionForHR(result);
            dialog.GetResult(out item);
            item.GetDisplayName(0x80058000, out path);
            return Marshal.PtrToStringUni(path);
        } finally {
            if (path != IntPtr.Zero) Marshal.FreeCoTaskMem(path);
            if (item != null) Marshal.FinalReleaseComObject(item);
            Marshal.FinalReleaseComObject(dialog);
        }
    }
    [ComImport, Guid("42F85136-DB7E-439C-85F1-E4075D135FC8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IFileDialog
    {
        [PreserveSig] int Show(IntPtr owner);
        void SetFileTypes(uint count, IntPtr types);
        void SetFileTypeIndex(uint index);
        void GetFileTypeIndex(out uint index);
        void Advise(IntPtr events, out uint cookie);
        void Unadvise(uint cookie);
        void SetOptions(uint options);
        void GetOptions(out uint options);
        void SetDefaultFolder(IShellItem folder);
        void SetFolder(IShellItem folder);
        void GetFolder(out IShellItem folder);
        void GetCurrentSelection(out IShellItem item);
        void SetFileName([MarshalAs(UnmanagedType.LPWStr)] string name);
        void GetFileName(out IntPtr name);
        void SetTitle([MarshalAs(UnmanagedType.LPWStr)] string title);
        void SetOkButtonLabel([MarshalAs(UnmanagedType.LPWStr)] string label);
        void SetFileNameLabel([MarshalAs(UnmanagedType.LPWStr)] string label);
        void GetResult(out IShellItem item);
    }
    [ComImport, Guid("43826D1E-E718-42EE-BC55-A1E261C37BFE"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellItem
    {
        void BindToHandler(IntPtr context, ref Guid handler, ref Guid id, out IntPtr result);
        void GetParent(out IShellItem parent);
        void GetDisplayName(uint kind, out IntPtr name);
    }
}
