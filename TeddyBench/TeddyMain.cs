using NAudio.Lame;
using NAudio.Wave;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using PdfSharp.Drawing;
using PdfSharp.Drawing.Layout;
using PdfSharp.Pdf;
using System;
using System.Collections;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.ComponentModel;
using System.Data;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Reflection;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using System.Windows.Forms;
using TeddyBench.Properties;
using TonieFile;
using static System.Windows.Forms.VisualStyles.VisualStyleElement;
using Application = System.Windows.Forms.Application;

namespace TeddyBench
{
    public partial class TeddyMain : Form
    {
        private SafeThread ScanCardThread = null;
        private bool ScanCardThreadStop = false;
        private SafeThread AnalyzeThread = null;
        private bool AnalyzeThreadStop = false;
        private SafeThread EncodeThread = null;
        private SafeThread UpdateCheckThread = null;

        private string CurrentDirectory = null;
        private string InternalContentDir = System.IO.Path.Combine(System.IO.Path.GetDirectoryName(Application.ExecutablePath), "Content");
        private bool AutoOpenDrive = true;
        private Dictionary<ListViewTag, ListViewItem> RegisteredItems = new Dictionary<ListViewTag, ListViewItem>();

        private static object TonieInfoLock = new object();
        private static TonieTools.TonieData[] TonieInfo = new TonieTools.TonieData[0];
        private static Dictionary<string, string> TonieInfoCustom = new Dictionary<string, string>();
        private static string TonieInfoString = "";

        private ConcurrentDictionary<string, Tuple<TonieAudio, DateTime>> CachedAudios = new ConcurrentDictionary<string, Tuple<TonieAudio, DateTime>>();
        private readonly object ImageCacheLock = new object();
        private ListViewItem LastSelectediItem = null;
        private RfidReaderBase RfidReader;
        private bool AutoSelected;
        private Settings Settings = null;
        internal LogWindow Log;
        private string LastFoundUid = null;
        private SafeThread AsyncTagActionThread = null;
        private System.Windows.Forms.Timer StatusBarTimer = null;

        private string TitleString => "TeddyBench - " + GetVersion();

        // absolute path to the bundled ffmpeg.exe, resolved next to the running exe rather than
        // relative to the process' current working directory (which isn't reliably the exe's own
        // folder, e.g. when launched from a shortcut/other tool with a different start-in folder).
        private static readonly string FfmpegPath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "Resources", "ffmpeg.exe");

        // default folder offered by the export/save file dialogs, next to the running exe
        private static readonly string DefaultExportDirectory = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "Export");

        /// <summary>
        /// Creates a FolderBrowserDialog pre-pointed at the "Export" directory next to the exe
        /// (created on demand), so the user doesn't have to navigate there by hand every time.
        /// Falls back to the dialog's normal default if that folder can't be created/accessed.
        /// </summary>
        private FolderBrowserDialog CreateExportFolderBrowserDialog()
        {
            FolderBrowserDialog dlg = new FolderBrowserDialog();

            try
            {
                if (!Directory.Exists(DefaultExportDirectory))
                {
                    Directory.CreateDirectory(DefaultExportDirectory);
                }

                dlg.SelectedPath = DefaultExportDirectory;
            }
            catch (Exception ex)
            {
                LogWindow.Log(LogWindow.eLogLevel.Warning, "[CreateExportFolderBrowserDialog] Failed to prepare default export directory '" + DefaultExportDirectory + "': " + ex.Message);
            }

            return dlg;
        }

        private SafeThread PlayThread = null;
        private bool PlayThreadStop = true;
        private bool TrackMouseDown = false;
        private int TrackNewPosition = -1;

        public class ListViewTag
        {
            public string FileName;
            public FileInfo FileInfo;
            public string Hash;
            public TonieTools.TonieData Info;
            public string Uid;
            public uint AudioId;

            public ListViewTag(string filename)
            {
                FileName = filename;

                FileInfo = new FileInfo(FileName);
                Uid = ReverseUid(FileInfo.Directory.Name + FileInfo.Name);
            }
        }

        public class ListViewItemComparer : IComparer
        {
            public int Characteristic = 0;
            public bool Order = true;

            public ListViewItemComparer(int ch)
            {
                Characteristic = ch;
            }

            public int Compare(object a, object b)
            {
                ListViewItem item1 = a as ListViewItem;
                ListViewItem item2 = b as ListViewItem;
                ListViewTag t1 = item1.Tag as ListViewTag;
                ListViewTag t2 = item2.Tag as ListViewTag;

                string s1 = "";
                string s2 = "";

                switch (Characteristic)
                {
                    case 0:
                        s1 = t1.Uid;
                        s2 = t2.Uid;
                        break;
                    case 1:
                        s1 = t1.Info?.Model;
                        s2 = t2.Info?.Model;
                        break;
                    case 2:
                        s1 = t1.FileInfo.CreationTime.ToString();
                        s2 = t2.FileInfo.CreationTime.ToString();
                        break;
                    case 3:
                        s1 = t1.Info?.SortString;
                        s2 = t2.Info?.SortString;
                        break;
                }

                if (s1 == null)
                {
                    return -1;
                }
                if (s2 == null)
                {
                    return 1;
                }

                int returnVal = String.Compare(s1, s2);

                return (Order ? 1 : -1) * returnVal;
            }
        }

        public void LoadJson(bool force = false)
        {
            try
            {
                string jsonContent = "";

                if (Settings.DownloadJson || force || !File.Exists("tonies.json"))
                {
                    try
                    {
                        TonieInfoString = "| Downloading...";
                        HttpWebRequest request = (HttpWebRequest)WebRequest.Create("https://api.revvox.de/tonies.json?source=TeddyBench&version=" + ThisAssembly.Git.BaseTag);
                        HttpWebResponse response = (HttpWebResponse)request.GetResponse();
                        TextReader reader = new StreamReader(response.GetResponseStream());
                        jsonContent = reader.ReadToEnd();
                        TonieInfoString = "| Downloaded";
                    }
                    catch (Exception e)
                    {
                        TonieInfoString = "| Download Failed";
                        ReportException("Downloader", e);
                        return;
                    }

                    if (string.IsNullOrEmpty(jsonContent))
                    {
                        return;
                    }
                    bool written = false;
                    for (int loop = 0; loop < 5; loop++)
                    {
                        try
                        {
                            File.WriteAllText("tonies.json", jsonContent);
                            written = true;
                            break;
                        }
                        catch (IOException e)
                        {
                            ReportException("Writing tonies.json", e);
                            Thread.Sleep(100);
                        }
                    }

                    if (!written)
                    {
                        TonieInfoString = "| Writing Failed - is the tonies.json accessible?";
                    }
                }
                else
                {
                    try
                    {
                        jsonContent = File.ReadAllText("tonies.json");
                    }
                    catch (Exception e)
                    {
                        TonieInfoString = "| Reading tonies.json Failed";
                        TonieInfo = new TonieTools.TonieData[0];
                    }
                }

                if(string.IsNullOrEmpty(jsonContent))
                {
                    /* failed to read or download. TonieInfoString is already set */
                    return;
                }

                lock (TonieInfoLock)
                {
                    try
                    {
                        TonieInfoString = "| Parsing...";
                        TonieInfo = JsonConvert.DeserializeObject<TonieTools.TonieData[]>(jsonContent);
                        TonieInfoString = "";
                    }
                    catch (Exception e)
                    {
                        TonieInfoString = "| Parsing Failed";
                        TonieInfo = new TonieTools.TonieData[0];
                        ReportException("Parse tonies.json", e);
                    }

                    try
                    {
                        TonieInfoCustom = JsonConvert.DeserializeObject<Dictionary<string, string>>(File.ReadAllText("customTonies.json"));
                    }
                    catch (FileNotFoundException e)
                    {
                        TonieInfoCustom.Clear();
                    }
                    catch (Exception e)
                    {
                        TonieInfoCustom.Clear();
                    }
                }
            }
            catch (Exception e)
            {
                return;
            }
        }

        void SaveJson()
        {
            lock (TonieInfoLock)
            {
                try
                {
                    File.WriteAllText("customTonies.json", JsonConvert.SerializeObject(TonieInfoCustom, Formatting.Indented));
                }
                catch (Exception e)
                {
                    return;
                }
            }
        }

        public TeddyMain()
        {
            InitializeComponent();
            Log = new LogWindow();

            Settings = Settings.FromFile("teddyBench.cfg");

            lstTonies.LargeImageList = new ImageList();
            lstTonies.LargeImageList.ImageSize = new Size(128, 128);
            lstTonies.LargeImageList.ColorDepth = ColorDepth.Depth32Bit;
            lstTonies.LargeImageList.Images.Add("unknown", ResizeImage(Resources.unknown, 128, 128));
            lstTonies.LargeImageList.Images.Add("custom", ResizeImage(Resources.custom, 128, 128));
            lstTonies.ListViewItemSorter = new ListViewItemComparer(2);
            lstTonies.DoubleBuffered(true);
            cmbSorting.SelectedIndex = 2;
            Text = TitleString;


            StartThreads();

            UpdateCheckThread = new SafeThread(UpdateCheck, "UpdateCheckThread");
            UpdateCheckThread.Start();

            autodetectionEnabledToolStripMenuItem.Checked = Settings.NfcEnabled;
            ReportForm.DefaultUsername = Settings.Username;
            UpdateNfcReader();

            AllowDrop = true;

            if (Settings.DebugWindow)
            {
                LogWindow.LogLevel = LogWindow.eLogLevel.DebugVerbose;
                enableDebugModeToolStripMenuItem.Checked = true;
            }
            if (Settings.DownloadJson)
            {
                downloadToniesjsonOnStartupToolStripMenuItem.Checked = true;
            }

            StatusBarTimer = new System.Windows.Forms.Timer();
            StatusBarTimer.Tick += (object sender, EventArgs e) => { UpdateStatusBar(); };
            StatusBarTimer.Interval = 250;
            StatusBarTimer.Start();

            // open internal content dir if exists
            if (System.IO.Directory.Exists(InternalContentDir))
            {
                OpenPath(InternalContentDir);
            }
        }

        void SaveSettings()
        {
            Settings.Save("teddyBench.cfg");
        }


        private void Proxmark3_FlashResult(object sender, bool e)
        {
            if (InvokeRequired)
            {
                BeginInvoke(new Action(() => Proxmark3_FlashResult(sender, e)));
                return;
            }

            if (e)
            {
                MessageBox.Show("Flashing the device succeeded, will reconnect now", "Flashing Proxmark3 done");
            }
            else
            {
                MessageBox.Show(
                    "Flashing the device FAILED, it will reconnect now." + Environment.NewLine +
                    "" + Environment.NewLine +
                    "If Device starts with LEDs A and C lit and it doesn't show" + Environment.NewLine +
                    "on USB, replug the device with the button pressed and *keep*" + Environment.NewLine +
                    "it pressed until it's flashed again." + Environment.NewLine +
                    "" + Environment.NewLine +
                    "If this still fails, please" + Environment.NewLine +
                    " a) check the firmware file or" + Environment.NewLine +
                    " b) use the official flasher tool"
                    , "Flashing Proxmark3 failed");
            }
        }

        private void Proxmark3_FlashRequest(object sender, Proxmark3.FlashRequestContext e)
        {
            if (InvokeRequired)
            {
                Invoke(new Action(() => Proxmark3_FlashRequest(sender, e)));
                return;
            }

            e.Proceed = false;

            if (e.Bootloader)
            {
                if (MessageBox.Show(
                    "=== FLASHING BOOTLOADER ===" + Environment.NewLine +
                    "" + Environment.NewLine +
                    "Please be aware that this procedure could fail and your device" + Environment.NewLine +
                    "gets bricked. To recover you need a special programmer." + Environment.NewLine +
                    "" + Environment.NewLine +
                    "Do you still want to flash the file " + e.FlashFile + "?",
                    "Flash Proxmark3?", MessageBoxButtons.YesNo) == DialogResult.Yes)
                {
                    e.Proceed = true;
                }
            }
            else
            {

                if (MessageBox.Show(
                    "Please be aware that this procedure could fail and your device" + Environment.NewLine +
                    "main firmware is lost. (Device starts with LEDs A and C lit and" + Environment.NewLine +
                    "it doesn't enumerate on USB anymore)." + Environment.NewLine +
                    "No panic, the bootloader wasn't touched and you can reflash it." + Environment.NewLine +
                    "" + Environment.NewLine +
                    "If so, plug the device with the button pressed and *keep* it" + Environment.NewLine +
                    "pressed until it's flashed again." + Environment.NewLine +
                    "I did this procedure several times and it always worked." + Environment.NewLine +
                    "" + Environment.NewLine +
                    "If this still fails, please" + Environment.NewLine +
                    " a) check the firmware file or" + Environment.NewLine +
                    " b) use the official flasher tool" + Environment.NewLine +
                    "" + Environment.NewLine +
                    "Do you want to flash the file " + e.FlashFile + "?",
                    "Flash Proxmark3?", MessageBoxButtons.YesNo) == DialogResult.Yes)
                {
                    e.Proceed = true;
                }
            }
        }

        private void Proxmark3_DeviceFound(object sender, string e)
        {
            if (InvokeRequired)
            {
                BeginInvoke(new Action(() => Proxmark3_DeviceFound(sender, e)));
                return;
            }

            if (e == null)
            {
                UpdateStatusBar();
                advancedActionsToolStripMenuItem.Enabled = false;
                reportProxmarkAnToolStripMenuItem.Enabled = false;
                reportNFCTagToolStripMenuItem.Enabled = false;
            }
            else
            {
                UpdateStatusBar();
                advancedActionsToolStripMenuItem.Enabled = RfidReader.UnlockSupported;
                reportProxmarkAnToolStripMenuItem.Enabled = true;
                reportNFCTagToolStripMenuItem.Enabled = true;

                flashBootloaderToolStripMenuItem.Enabled = (RfidReader.DeviceInfo & RfidReaderBase.eDeviceInfo.BootromPresent) != 0;
                flashFirmwareToolStripMenuItem.Enabled = (RfidReader.DeviceInfo & RfidReaderBase.eDeviceInfo.BootromPresent) != 0;
            }
        }

        private void UpdateStatusBar()
        {
            string text = "";

            if (RfidReader != null && RfidReader.Connected)
            {
                if (!statusStrip1.Visible)
                {
                    statusStrip1.Show();
                }
                string voltageString = Settings.DebugWindow ? ", " + RfidReader.AntennaVoltage.ToString("0.00", CultureInfo.InvariantCulture) + " V" : "";

                text += " | " + RfidReader.HardwareType + " (FW: " + (RfidReader.UnlockSupported ? "SLIX-L enabled" : "stock") + voltageString + ") found at " + RfidReader.CurrentPort + ". The UID of the tag will be automatically used where applicable.";
            }

            lock (TonieInfoLock)
            {
                if (TonieInfo.Length == 0)
                {
                    text += " | No tonies.json loaded, consider downloading it";
                }
                else
                {
                    text += " | Have " + TonieInfo.Length + " entries in tonies.json";
                }
                if (!string.IsNullOrEmpty(TonieInfoString))
                {
                    text += " " + TonieInfoString;
                }
            }

            if (text == "")
            {
                if (statusStrip1.Visible)
                {
                    statusStrip1.Hide();
                }
            }
            else
            {
                if (!statusStrip1.Visible)
                {
                    statusStrip1.Show();
                }
                statusLabel.Text = text.Trim().Trim('|').Trim();
            }
        }

        private void Proxmark3_UidFound(object sender, string e)
        {
            if (InvokeRequired)
            {
                BeginInvoke(new Action(() => Proxmark3_UidFound(sender, e)));
                return;
            }

            bool found = false;

            if (e != null)
            {
                /* already handled this UID? then return */
                if (e == LastFoundUid)
                {
                    return;
                }

                foreach (ListViewItem item in lstTonies.Items)
                {
                    ListViewTag tag = item.Tag as ListViewTag;

                    if (tag.Uid == e)
                    {
                        if (lstTonies.SelectedItems.Count == 1 && lstTonies.SelectedItems[0] == item)
                        {
                            return;
                        }
                        foreach (ListViewItem sel in lstTonies.SelectedItems)
                        {
                            sel.Selected = false;
                        }
                        item.Selected = true;
                        item.EnsureVisible();
                        ActiveControl = lstTonies;
                        found = true;
                        AutoSelected = true;
                    }
                }
            }

            LastFoundUid = e;

            /* tag was removed and the selected item was auto selected because tag was found, deselect again */
            if (!found && AutoSelected)
            {
                foreach (ListViewItem sel in lstTonies.SelectedItems)
                {
                    sel.Selected = false;
                }
            }
        }

        public async void UpdateCheck()
        {
            try
            {
                string thisVersion = ThisAssembly.Git.BaseTag;
                if (!string.IsNullOrEmpty(thisVersion))
                {
                    Thread.Sleep(2000);

                    HttpClient client = new HttpClient();
                    client.DefaultRequestHeaders.UserAgent.ParseAdd("Mozilla/5.0 (compatible; TeddyBench/1.0)");
                    async Task<JObject> GithubApiGet(string path) => JObject.Parse(await client.GetStringAsync($"https://api.github.com/{path}"));
                    async Task<JObject> GithubLastRelease(string user, string repo) => await GithubApiGet($"repos/{user}/{repo}/releases/latest");

                    async Task DownloadFile(string url, string destinationFileName)
                    {
                        using (var stream = await client.GetStreamAsync(url))
                        {
                            using (var file = new FileStream(destinationFileName, FileMode.Create))
                            {
                                stream.CopyTo(file);
                            }
                        }
                    }

                    dynamic latestRelease = await GithubLastRelease("StefanRiese", "teddy");
                    string latestVersion = latestRelease.tag_name;

                    if (latestVersion != thisVersion)
                    {
                        string destPath = new FileInfo(Assembly.GetExecutingAssembly().Location).DirectoryName;
                        string zipName = Path.Combine(destPath, "TeddyBench.zip");

                        if (latestRelease.assets[0].name == "TeddyBench.zip")
                        {
                            string url = latestRelease.assets[0].browser_download_url;

                            BeginInvoke(new Action(async () =>
                            {
                                UpdateNotifyDialog dlg = new UpdateNotifyDialog(latestVersion + " (yours: " + thisVersion + ")", (string)latestRelease.name);
                                if (dlg.ShowDialog() == DialogResult.Yes)
                                {
                                    await DownloadFile(url, zipName);
                                    ProcessStartInfo startInfo = new ProcessStartInfo
                                    {
                                        Arguments = zipName,
                                        FileName = "explorer.exe"
                                    };

                                    Process.Start(startInfo);
                                }
                            }));
                        }
                    }
                }
            }
            catch (Exception ex)
            {
            }
        }

        protected override void OnDragEnter(DragEventArgs e)
        {
            if (e.Data.GetDataPresent(DataFormats.FileDrop))
                e.Effect = DragDropEffects.Copy;
            else
                e.Effect = DragDropEffects.None;
        }

        protected override void OnDragDrop(DragEventArgs drgevent)
        {
            string[] drop = drgevent.Data.GetData(DataFormats.FileDrop) as string[];

            if (drop != null)
            {
                AddFiles(drop, GetAudioID());
            }
        }

        private uint GetAudioID()
        {
            if (MessageBox.Show("Do you want to set a specific Audio-ID? If you don't know, just say 'No'.", "Set specific Audio ID", MessageBoxButtons.YesNo) == DialogResult.No)
            {
                return uint.MaxValue;
            }

            AskHexForm form = new AskHexForm();
            if (form.ShowDialog() != DialogResult.OK)
            {
                return uint.MaxValue;
            }

            return form.Value;
        }

        protected override void OnFormClosing(FormClosingEventArgs e)
        {
            StopThreads();
            SaveSettings();
            base.OnFormClosing(e);
        }

        private void StopThreads()
        {
            if (ScanCardThread != null)
            {
                ScanCardThreadStop = true;
                ScanCardThread.Join(100);
                ScanCardThread.Abort();
                ScanCardThread = null;
            }
            if (PlayThread != null)
            {
                PlayThreadStop = true;
                PlayThread.Join(1000);
                PlayThread.Abort();
                PlayThread = null;
            }
            StopAnalyzeThread();
            if (UpdateCheckThread != null)
            {
                UpdateCheckThread.Join(100);
                UpdateCheckThread.Abort();
                UpdateCheckThread = null;
            }
            if (EncodeThread != null)
            {
                EncodeThread.Join(100);
                EncodeThread.Abort();
                EncodeThread = null;
            }
            if (RfidReader != null)
            {
                RfidReader.Stop();
                RfidReader = null;
            }
        }

        private void StartThreads()
        {
            ScanCardThreadStop = false;
            ScanCardThread = new SafeThread(ScanCardMain, "ScanCardThread");
            ScanCardThread.Start();


            new SafeThread(() =>
            {
                LoadJson();
            }, "JSON Downloader").Start();
        }

        private void ScanCardMain()
        {
            while (!ScanCardThreadStop)
            {
                try
                {
                    var drives = DriveInfo.GetDrives();

                    foreach (var drive in drives)
                    {
                        if (drive.DriveType == DriveType.Removable)
                        {
                            try
                            {
                                if (drive.IsReady && drive.RootDirectory.GetDirectories().Where(d => d.Name == "CONTENT").Count() == 1)
                                {
                                    NotifyDrive(drive);
                                }
                            }
                            catch (Exception ex)
                            {
                            }
                        }
                    }

                    if (CurrentDirectory != null)
                    {
                        bool failedRead = false;

                        try
                        {
                            if (!new DirectoryInfo(CurrentDirectory).Exists)
                            {
                                failedRead = true;
                            }
                        }
                        catch (Exception ex)
                        {
                            failedRead = true;
                        }

                        if (failedRead)
                        {
                            NotifyDriveLost();
                        }
                    }
                }
                catch (Exception ex)
                {
                }

                Thread.Sleep(500);
            }
        }

        private void NotifyDriveLost()
        {
            if (this.InvokeRequired)
            {
                this.BeginInvoke(new Action(() => { NotifyDriveLost(); }));
                return;
            }
            grpCardContent.Visible = false;
            lblMessage.Visible = true;
            CurrentDirectory = null;
            AutoOpenDrive = true;
            Text = TitleString;
            StopAnalyzeThread();
        }

        private void NotifyDrive(DriveInfo drive)
        {
            if (this.InvokeRequired)
            {
                this.BeginInvoke(new Action(() => { NotifyDrive(drive); }));
                return;
            }

            if (AutoOpenDrive)
            {
                try
                {
                    OpenPath(Path.Combine(drive.RootDirectory.FullName, "CONTENT"));
                    AutoOpenDrive = false;
                }
                catch (Exception ex)
                {
                    LogWindow.Log(LogWindow.eLogLevel.Error, "Wanted to open CONTENT on " + drive.RootDirectory + " but that caused a: " + ex.Message);
                    Thread.Sleep(1000);
                }
            }
        }

        private void openDirectoryToolStripMenuItem_Click(object sender, EventArgs e)
        {
            FolderBrowserDialog dlg = new FolderBrowserDialog();

            if (dlg.ShowDialog() == DialogResult.OK)
            {
                OpenPath(dlg.SelectedPath);
            }
        }

        private void OpenPath(string rootDirectory)
        {
            if (!Directory.Exists(rootDirectory))
            {
                CurrentDirectory = null;
                return;
            }

            CurrentDirectory = rootDirectory;

            FileSystemWatcher watcher = new FileSystemWatcher(CurrentDirectory);

            watcher.Changed += OnDirectoryChanged;
            watcher.Created += OnDirectoryChanged;
            watcher.Deleted += OnDirectoryChanged;
            watcher.Renamed += OnDirectoryChanged;

            watcher.EnableRaisingEvents = true;
            RefreshCardContent();
        }

        private void OnDirectoryChanged(object sender, FileSystemEventArgs e)
        {
            RefreshCardContent();
        }

        private void RefreshCardContent()
        {
            if (this.InvokeRequired)
            {
                this.BeginInvoke(new Action(() => { RefreshCardContent(); }));
                return;
            }

            StopAnalyzeThread();

            Text = TitleString + " - " + CurrentDirectory;

            lock (RegisteredItems)
            {
                btnAdd.Enabled = true;
                btnDelete.Enabled = true;
                btnSave.Enabled = true;

                txtLog.Visible = false;
                grpCardContent.Visible = true;
                lblMessage.Visible = false;
                lstTonies.Items.Clear();
                RegisteredItems.Clear();

                try
                {
                    /* scan card content */
                    foreach (var dir in new DirectoryInfo(CurrentDirectory).GetDirectories())
                    {
                        var file = dir.GetFiles("*0304E0").FirstOrDefault();

                        if (file != null)
                        {
                            ListViewItem item = new ListViewItem();
                            ListViewTag tag = new ListViewTag(file.FullName);

                            item.Tag = tag;
                            item.Text = tag.Uid;
                            item.ImageKey = "unknown";
                            item.Group = lstTonies.Groups[3];

                            lstTonies.Items.Add(item);

                            RegisteredItems.Add(item.Tag as ListViewTag, item);
                        }
                    }

                    /* scan local directory */
                    foreach (var file in new DirectoryInfo(CurrentDirectory).GetFiles())
                    {
                        Regex reg = new Regex("(?<prod>[0-9]{8}|[0-9]{2}-[0-9]{4}) - [0-9A-F]{8} - (?<name>.*)");
                        var match = reg.Match(file.Name);
                        if (match.Success)
                        {
                            ListViewItem item = new ListViewItem();
                            ListViewTag tag = new ListViewTag(file.FullName);

                            item.Tag = tag;
                            item.Text = match.Groups["prod"].Value + " - " + match.Groups["name"].Value;
                            item.ImageKey = "unknown";
                            item.Group = lstTonies.Groups[3];

                            lstTonies.Items.Add(item);

                            RegisteredItems.Add(item.Tag as ListViewTag, item);
                        }
                    }
                }
                catch (Exception ex)
                {
                    LogWindow.Log(LogWindow.eLogLevel.Error, "Exception while scanning directory '" + CurrentDirectory + "':" + ex.Message);
                }
                lstTonies.Sort();
            }

            StartAnalyzeThread();
        }

        private void StartAnalyzeThread()
        {
            AnalyzeThreadStop = false;
            AnalyzeThread = new SafeThread(AnalyzeMain, "AnalyzeThread");
            AnalyzeThread.Start();
        }

        private void StopAnalyzeThread()
        {
            if (AnalyzeThread != null)
            {
                AnalyzeThreadStop = true;
                if (!AnalyzeThread.Join(2000))
                {
                    LogWindow.Log(LogWindow.eLogLevel.Error, "Failed to stop Analyze Thread");
                }
                AnalyzeThread.Abort();
                AnalyzeThread = null;
            }
        }

        private void AnalyzeMain()
        {
            ByteQueue ByteBuffer = new ByteQueue();
            while (!AnalyzeThreadStop)
            {
                Thread.Sleep(100);

                lock (RegisteredItems)
                {
                    LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "Rescanning");
                    Parallel.ForEach(RegisteredItems, new ParallelOptions { MaxDegreeOfParallelism = 4 }, entry =>
                    {
                        ListViewTag tag = entry.Key;
                        LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "  Filename: " + entry.Key.FileName);
                        LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     Image: " + entry.Value.ImageKey);

                        if (entry.Value.ImageKey == "unknown")
                        {
                            try
                            {
                                LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "GetTonieAudio...");
                                TonieAudio dumpFile = GetTonieAudio(tag.FileName);
                                LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "GetTonieAudio... done");
                                string image = "";
                                string hash = BitConverter.ToString(dumpFile.Header.Hash).Replace("-", "");
                                IEnumerable<TonieTools.TonieData> found = new TonieTools.TonieData[0];

                                lock (TonieInfoLock)
                                {
                                    found = TonieInfo.Where(t => t.Hash.Where(h => h == hash).Count() > 0);
                                }
                                string tonieName = "[" + tag.Uid + "]" + Environment.NewLine + "(unknown: " + dumpFile.Header.AudioId.ToString("X8") + ")";

                                bool update = false;

                                if (found.Count() > 0)
                                {
                                    LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     Found");
                                    var info = found.First();
                                    tag.Info = info;
                                    tonieName = info.Title;

                                    if (!string.IsNullOrEmpty(info.Model))
                                    {
                                        tonieName = info.Model + " - " + tonieName;
                                    }
                                    lock (TonieInfoLock)
                                    {
                                        if (TonieInfoCustom.ContainsKey(hash))
                                        {
                                            LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     known tonie, overriding name");
                                            tonieName = TonieInfoCustom[hash];
                                        }
                                    }
                                    if (!string.IsNullOrEmpty(info.Pic))
                                    {
                                        lock (ImageCacheLock)
                                        {
                                            if (!lstTonies.LargeImageList.Images.ContainsKey(hash))
                                            {
                                                Image img = GetImage(info.Pic, hash);
                                                if (img != null)
                                                {
                                                    this.BeginInvoke(new Action(() =>
                                                    {
                                                        lstTonies.LargeImageList.Images.Add(hash, img);
                                                    }));
                                                }
                                            }
                                        }
                                    }
                                    update = true;
                                    image = hash;
                                }
                                else
                                {
                                    tag.Info = new TonieTools.TonieData();
                                    image = "unknown";

                                    lock (TonieInfoLock)
                                    {
                                        if (TonieInfoCustom.ContainsKey(hash))
                                        {
                                            LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     known custom tonie");
                                            tonieName = TonieInfoCustom[hash];
                                            tag.Info.Title = tonieName;
                                            image = "custom";
                                            update = true;
                                        }
                                        else if (dumpFile.Header.AudioId < 0x50000000)
                                        {
                                            LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     unknown custom tonie");
                                            tonieName = "Unnamed Teddy - " + tonieName;
                                            tag.Info.Title = "Unnamed Teddy";
                                            image = "custom";
                                            update = true;
                                        }
                                        else
                                        {
                                            LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     Not found -> unknown");
                                        }
                                    }
                                }

                                tag.Hash = hash;
                                tag.AudioId = dumpFile.Header.AudioId;
                                LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     Hash: " + tag.Hash);
                                LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     AudioId: " + tag.AudioId);

                                bool live = tag.FileInfo.Attributes.HasFlag(FileAttributes.Hidden);
                                string newText = (live ? "[live] " : "") + tonieName;
                                string newImakeKey = image;
                                string newToolTipText =
                                    "File:     " + tag.FileName + Environment.NewLine +
                                    "Name:     " + tag.Info.Title + Environment.NewLine +
                                    "UID:      " + tag.Uid + Environment.NewLine +
                                    "Date:     " + tag.FileInfo.CreationTime + Environment.NewLine +
                                    "AudioID:  0x" + tag.AudioId.ToString("X8") + Environment.NewLine +
                                    "Chapters: " + dumpFile.Header.AudioChapters.Length + Environment.NewLine;

                                update |= entry.Value.Text != newText;
                                update |= entry.Value.ImageKey != newImakeKey;
                                update |= entry.Value.ToolTipText != newToolTipText;

                                if (update)
                                {
                                    this.BeginInvoke(new Action(() =>
                                    {
                                        LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "     Update list");
                                        entry.Value.Text = newText;
                                        entry.Value.ImageKey = newImakeKey;
                                        entry.Value.ToolTipText = newToolTipText;
                                        UpdateSorting();
                                    }));
                                }
                            }
                            catch (Exception ex)
                            {
                            }
                        }
                    });
                    LogWindow.Log(LogWindow.eLogLevel.DebugVerbose, "Done");
                }
            }
        }

        private Image GetImage(string pic, string hash)
        {
            string cacheFileName = Path.Combine("cache", hash + ".png");

            try
            {
                if (!Directory.Exists("cache"))
                {
                    Directory.CreateDirectory("cache");
                }

                if (File.Exists(cacheFileName))
                {
                    return Image.FromFile(cacheFileName);
                }
            }
            catch (Exception ex)
            {
            }

            try
            {
                HttpWebRequest request = (HttpWebRequest)WebRequest.Create(pic);
                HttpWebResponse response = (HttpWebResponse)request.GetResponse();

                Image img;


                Image originalImage = Image.FromStream(response.GetResponseStream());

                if (originalImage.Height > originalImage.Width)
                {
                    originalImage = CropCenterOfImage(originalImage, new Size(originalImage.Width, originalImage.Width));
                }
                else
                {
                    originalImage = CropCenterOfImage(originalImage, new Size(originalImage.Height, originalImage.Height));
                }

                img = ResizeImage(originalImage, Settings.CacheImageSize, Settings.CacheImageSize);

                try
                {
                    img.Save(cacheFileName);
                }
                catch (Exception ex)
                {
                }
                return img;
            }
            catch (Exception ex)
            {
            }

            return null;
        }

        /// <summary>
        /// Stores the tonies.json cover art as "cover.<ext>" next to the exported audio files, so
        /// media players/managers can pick it up as folder art. Checks a persistent full-resolution
        /// cache (cache/covers/&lt;hash&gt;.&lt;ext&gt;) before hitting the network at all, since the same
        /// cover otherwise gets re-downloaded from scratch for every export batch/output folder -
        /// the per-directory "already present" check below only helps when re-exporting into the
        /// exact same folder. On a cache miss, downloads the full-resolution picUrl and populates
        /// that cache for next time; if the download fails for any reason (no network, broken URL,
        /// timeout, ...), falls back to the thumbnail already cached locally for the list view
        /// (cache/&lt;hash&gt;.png) - lower resolution (cropped/resized square), but better than no
        /// cover at all. By default an existing cover.&lt;ext&gt; in THIS output directory is left
        /// alone (so resuming/repeating a batch doesn't redo work); pass overwrite:true to bypass
        /// both that check and the full-resolution cache and force a fresh download (used by the
        /// "Update covers..." maintenance action). Returns a short human-readable outcome, so
        /// callers can log/display what happened instead of it failing silently, or null if the
        /// cover was already present and nothing was done.
        /// </summary>
        private string SaveCoverImage(string picUrl, string hash, string outDirectory, bool overwrite = false)
        {
            string coverFileName = "<unknown>";

            try
            {
                string extension = Path.GetExtension(new Uri(picUrl).LocalPath);

                if (string.IsNullOrEmpty(extension))
                {
                    extension = ".jpg";
                }

                coverFileName = Path.Combine(outDirectory, "cover" + extension);

                // an empty file (e.g. left behind by an interrupted write) does not count as present
                if (IsNonEmptyFile(coverFileName) && !overwrite)
                {
                    return null;
                }

                string fullResCacheDir = Path.Combine("cache", "covers");
                string fullResCacheFileName = Path.Combine(fullResCacheDir, hash + extension);

                if (!overwrite && IsNonEmptyFile(fullResCacheFileName))
                {
                    File.Copy(fullResCacheFileName, coverFileName, true);
                    DeleteOtherCoverFiles(outDirectory, coverFileName);
                    return "reused cached cover (" + fullResCacheFileName + ")";
                }

                // download into a temporary file first, so a broken-off download never leaves a
                // truncated cover behind that later exports would mistake for a complete one
                string tempFileName = coverFileName + ".part";

                try
                {
                    HttpWebRequest request = (HttpWebRequest)WebRequest.Create(picUrl);

                    using (HttpWebResponse response = (HttpWebResponse)request.GetResponse())
                    using (Stream responseStream = response.GetResponseStream())
                    using (FileStream outFile = File.Create(tempFileName))
                    {
                        responseStream.CopyTo(outFile);
                    }

                    if (!IsNonEmptyFile(tempFileName))
                    {
                        throw new IOException("server returned an empty image");
                    }

                    if (File.Exists(coverFileName))
                    {
                        File.Delete(coverFileName);
                    }
                    File.Move(tempFileName, coverFileName);
                    DeleteOtherCoverFiles(outDirectory, coverFileName);

                    try
                    {
                        if (!Directory.Exists(fullResCacheDir))
                        {
                            Directory.CreateDirectory(fullResCacheDir);
                        }
                        File.Copy(coverFileName, fullResCacheFileName, true);
                    }
                    catch (Exception)
                    {
                        // best-effort - the export itself already succeeded without the cache
                    }

                    return "downloaded (" + coverFileName + ")";
                }
                catch (Exception downloadEx)
                {
                    try { File.Delete(tempFileName); } catch { }

                    // with overwrite, keep the cover that is already there rather than replacing it
                    // with the low-resolution thumbnail
                    if (IsNonEmptyFile(coverFileName))
                    {
                        return "download failed (" + downloadEx.Message + "), kept existing cover (" + coverFileName + ")";
                    }

                    // download failed - fall back to the thumbnail already cached for the list view.
                    // that cache is always PNG, whatever format the original cover has.
                    string cachedThumbnail = Path.Combine("cache", hash + ".png");

                    if (File.Exists(cachedThumbnail))
                    {
                        string thumbnailCoverFileName = Path.Combine(outDirectory, "cover.png");
                        File.Copy(cachedThumbnail, thumbnailCoverFileName, true);
                        return "download failed (" + downloadEx.Message + "), used local thumbnail cache instead (" + thumbnailCoverFileName + ")";
                    }

                    throw;
                }
            }
            catch (Exception ex)
            {
                string message = "FAILED to save cover image '" + picUrl + "' to '" + coverFileName + "': " + ex.Message;
                LogWindow.Log(LogWindow.eLogLevel.Warning, "[SaveCoverImage] " + message);
                return message;
            }
        }

        private static readonly string[] CoverExtensions = { ".jpg", ".jpeg", ".png" };

        private static bool IsNonEmptyFile(string fileName)
        {
            return File.Exists(fileName) && new FileInfo(fileName).Length > 0;
        }

        /// <summary>
        /// Returns the cover.&lt;ext&gt; saved by SaveCoverImage in this directory, or null if there is none.
        /// </summary>
        private static string FindCoverFile(string directory)
        {
            return CoverExtensions.Select(ext => Path.Combine(directory, "cover" + ext)).FirstOrDefault(IsNonEmptyFile);
        }

        /// <summary>
        /// Removes stale covers with a different extension, e.g. a thumbnail fallback cover.png
        /// once the full-resolution cover.jpg could be downloaded.
        /// </summary>
        private static void DeleteOtherCoverFiles(string directory, string keepFileName)
        {
            foreach (string ext in CoverExtensions)
            {
                string fileName = Path.Combine(directory, "cover" + ext);

                if (!string.Equals(fileName, keepFileName, StringComparison.OrdinalIgnoreCase) && File.Exists(fileName))
                {
                    try { File.Delete(fileName); } catch { }
                }
            }
        }

        private TonieAudio GetTonieAudio(string fileName)
        {
            FileInfo info = new FileInfo(fileName);
            if (CachedAudios.TryGetValue(fileName, out Tuple<TonieAudio, DateTime> cachedItem) && cachedItem.Item2 == info.LastWriteTime)
            {
                return cachedItem.Item1;
            }

            TonieAudio file = TonieAudio.FromFile(fileName, false);
            CachedAudios[fileName] = new Tuple<TonieAudio, DateTime>(file, info.LastWriteTime);

            return file;
        }

        public static Bitmap ResizeImage(Image image, int width, int height)
        {
            double srcRatio = (double)image.Width / image.Height;
            int dstWidth = width;
            int dstHeight = (int)(dstWidth / srcRatio);

            if (dstHeight > image.Height)
            {
                dstHeight = height;
                dstWidth = (int)(dstHeight * srcRatio);
            }

            var destRect = new Rectangle(0, 0, dstWidth, dstHeight);
            var destImage = new Bitmap(width, height);

            destImage.SetResolution(image.HorizontalResolution, image.VerticalResolution);

            using (var graphics = Graphics.FromImage(destImage))
            {
                graphics.CompositingMode = CompositingMode.SourceCopy;
                graphics.CompositingQuality = CompositingQuality.HighQuality;
                graphics.InterpolationMode = InterpolationMode.HighQualityBicubic;
                graphics.SmoothingMode = SmoothingMode.HighQuality;
                graphics.PixelOffsetMode = PixelOffsetMode.HighQuality;

                using (var wrapMode = new ImageAttributes())
                {
                    wrapMode.SetWrapMode(WrapMode.TileFlipXY);
                    graphics.DrawImage(image, destRect, 0, 0, image.Width, image.Height, GraphicsUnit.Pixel, wrapMode);
                }
            }

            return destImage;
        }

        private void btnAdd_Click(object sender, EventArgs e)
        {
            OpenFileDialog dlg = new OpenFileDialog();
            dlg.Multiselect = true;

            if (dlg.ShowDialog() == DialogResult.OK)
            {
                AddFiles(dlg.FileNames, GetAudioID());
            }
        }

        private void AddFiles(string[] fileNames, uint id = uint.MaxValue)
        {
            AskUIDForm ask = new AskUIDForm(RfidReader);

            if (ask.ShowDialog() == DialogResult.OK)
            {
                if (fileNames.Count() == 1)
                {
                    string fileName = fileNames[0];

                    if (fileName.ToLower().EndsWith(".mp3") || fileName.ToLower().EndsWith(".ogg"))
                    {
                        switch (MessageBox.Show("You are about to encode a single MP3/Ogg, is this right?", "Encode a file", MessageBoxButtons.YesNo))
                        {
                            case DialogResult.No:
                                return;
                            case DialogResult.Yes:
                                EncodeFile(ask.Uid, new[] { fileName }, id);
                                return;
                        }
                    }
                    else
                    {
                        try
                        {
                            TonieAudio dumpFile = TonieAudio.FromFile(fileName);

                            if (dumpFile.FileContent.Length > 0)
                            {
                                CopyFile(ask.Uid, fileName);
                            }
                        }
                        catch (Exception ex)
                        {
                            MessageBox.Show("The file you have chosen is not supported.", "Add file...");
                            return;
                        }
                    }
                }
                else
                {
                    if (fileNames.Where(f => !(f.ToLower().EndsWith(".mp3") || f.ToLower().EndsWith(".ogg"))).Count() > 0)
                    {
                        MessageBox.Show("Please select MP3/Ogg files only.", "Add file...");
                        return;
                    }

                    TrackSortDialog sorter = new TrackSortDialog(fileNames);

                    if(sorter.ShowDialog() == DialogResult.Cancel)
                    {
                        return;
                    }

                    string[] sorted = sorter.SortedFiles;

                    EncodeFile(ask.Uid, sorted, id);
                }
            }
        }

        private void CopyFile(string uid, string fileName)
        {
            btnAdd.Enabled = false;
            btnDelete.Enabled = false;
            btnSave.Enabled = false;

            EncodeThread = new SafeThread(() =>
            {
                string newDir = Path.Combine(CurrentDirectory, ReverseUid(uid).Substring(0, 8));
                string newFile = Path.Combine(newDir, ReverseUid(uid).Substring(8, 8));

                try
                {
                    Directory.CreateDirectory(newDir);
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to create directory '" + newDir + "'");
                    return;
                }

                try
                {
                    File.WriteAllBytes(newFile, File.ReadAllBytes(fileName));
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to write file '" + newFile + "'");
                    return;
                }

                RefreshCardContent();
            }, "EncodeThread");
            EncodeThread.Start();
        }

        public class EncodeCallback : TonieAudio.EncodeCallback
        {
            private readonly TeddyMain Main;
            private int LastPct = 0;

            public EncodeCallback(TeddyMain main)
            {
                Main = main;
            }

            public override void FileStart(int track, string sourceFile)
            {
                ParseName(track, sourceFile);
                Main.BeginInvoke(new Action(() => { Main.txtLog.Text += " File: " + ShortName; }));
            }

            public override void FileDone()
            {
                Main.BeginInvoke(new Action(() => { Main.txtLog.Text += "]" + Environment.NewLine; }));
            }

            public override void FileFailed(string message)
            {
                Main.BeginInvoke(new Action(() => { Main.txtLog.Text += "] FAILED" + Environment.NewLine; }));
            }

            public override void Progress(decimal pct)
            {
                LastPct = (int)(pct * 20);
                if (LastPct % 5 == 0)
                {
                    if (LastPct != 20)
                    {
                        Main.BeginInvoke(new Action(() => { Main.txtLog.Text += "" + (LastPct * 5) + "%"; }));
                    }
                }
                else
                {
                    Main.BeginInvoke(new Action(() => { Main.txtLog.Text += "."; }));
                }
            }

            public override void Failed(string message)
            {
                Main.BeginInvoke(new Action(() => { Main.txtLog.Text += "] FAILED" + Environment.NewLine; }));
            }

            public override void Warning(string message)
            {
                Main.BeginInvoke(new Action(() => { Main.txtLog.Text += Environment.NewLine + "Warning" + Environment.NewLine; }));
            }
        }

        private void EncodeFile(string uid, string[] filenames, uint id = uint.MaxValue)
        {
            btnAdd.Enabled = false;
            btnDelete.Enabled = false;
            btnSave.Enabled = false;

            grpCardContent.Visible = false;
            txtLog.Visible = true;

            EncodeThread = new SafeThread(() =>
            {
                string newDir = Path.Combine(CurrentDirectory, ReverseUid(uid).Substring(0, 8));
                string newFile = Path.Combine(newDir, ReverseUid(uid).Substring(8, 8));

                TonieAudio audio = null;

                try
                {
                    if (id == uint.MaxValue)
                    {
                        id = (uint)(DateTimeOffset.Now.ToUnixTimeSeconds() - 0x50000000);
                    }
                    audio = new TonieAudio(filenames, id, cbr: new EncodeCallback(this));
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to encode file. Sorry. Most likely a MP3 was corrupted." + Environment.NewLine + "(" + ex.Message + ")");
                    return;
                }

                try
                {
                    Directory.CreateDirectory(newDir);
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to create directory '" + newDir + "'");
                    return;
                }
                try
                {
                    File.WriteAllBytes(newFile, audio.FileContent);
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to write file '" + newFile + "'");
                    return;
                }

                RefreshCardContent();
            }, "EncodeThread");
            EncodeThread.Start();
        }


        public static string ReverseUid(string uid)
        {
            List<string> groups = (from Match m in Regex.Matches(uid, @"[A-Fa-f0-9]{2}") select m.Value).ToList();
            groups.Reverse();
            string ret = string.Join("", groups.ToArray());

            return ret;
        }

        private void aboutToolStripMenuItem_Click(object sender, EventArgs e)
        {
            AboutDialog dlg = new AboutDialog();
            dlg.ShowDialog();
        }

        private void btnDelete_Click(object sender, EventArgs e)
        {
        }



        #region Context menu for tonie files


        private void renameToolStripMenuItem_Click(object sender, EventArgs e)
        {
            if (LastSelectediItem != null/* && LastSelectediItem.ImageKey == "custom"*/)
            {
                LastSelectediItem.BeginEdit();
            }
        }

        private void deleteToolStripMenuItem_Click(object sender, EventArgs e)
        {
            DeleteSelected();
        }

        private async void exportTooggToolStripMenuItem_Click(object sender, EventArgs e)
        {
            await SaveSelected();
        }

        private async void exportToMp3ToolStripMenuItem_Click(object sender, EventArgs e)
        {
            await SaveSelected(true);
        }

        private void exportToToolStripMenuItem_Click(object sender, EventArgs e)
        {
            ExportSelected();
        }

        private void toolStripMenuItem4_Click(object sender, EventArgs e)
        {
            FolderBrowserDialog dlg = new FolderBrowserDialog();

            if (dlg.ShowDialog() != DialogResult.OK)
            {
                return;
            }
            string outputLocation = dlg.SelectedPath;

            foreach (ListViewItem item in lstTonies.SelectedItems)
            {
                ListViewTag tag = item.Tag as ListViewTag;

                try
                {
                    string destName = Path.Combine(outputLocation, Path.GetFileName(Path.GetDirectoryName(tag.FileName)));
                    CopyDirectory(Path.GetDirectoryName(tag.FileName), destName);
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to copy directory '" + tag.FileName + "': " + ex.Message);
                    return;
                }
            }

            MessageBox.Show("Finished");
        }

        public static void CopyDirectory(string sourceDirectory, string targetDirectory)
        {
            var diSource = new DirectoryInfo(sourceDirectory);
            var diTarget = new DirectoryInfo(targetDirectory);

            CopyAll(diSource, diTarget);
        }

        public static void CopyAll(DirectoryInfo source, DirectoryInfo target)
        {
            Directory.CreateDirectory(target.FullName);

            // Copy each file into the new directory.
            foreach (FileInfo fi in source.GetFiles())
            {
                // do not copy if file already exists
                if (!System.IO.File.Exists(Path.Combine(target.FullName, fi.Name)))
                { 
                    fi.CopyTo(Path.Combine(target.FullName, fi.Name), true);
                }
            }

            // Copy each subdirectory using recursion.
            foreach (DirectoryInfo diSourceSubDir in source.GetDirectories())
            {
                DirectoryInfo nextTargetSubDir =
                    target.CreateSubdirectory(diSourceSubDir.Name);
                CopyAll(diSourceSubDir, nextTargetSubDir);
            }
        }

        private void assignNewUIDToolStripMenuItem_Click(object sender, EventArgs e)
        {
            ReassignSelected();
        }

        private void showInExplorerToolStripMenuItem_Click(object sender, EventArgs e)
        {
            if (LastSelectediItem != null)
            {
                ListViewTag tag = LastSelectediItem.Tag as ListViewTag;
                System.Diagnostics.Process.Start("explorer.exe", "/select, \"" + tag.FileName + "\"");
            }
        }

        private async void sendDiagnosticsReportToolStripMenuItem_Click(object sender, EventArgs e)
        {
            await ReportSelected();
        }

        #endregion

        #region ListView callbacks

        private void lstTonies_MouseClick(object sender, MouseEventArgs e)
        {
            if (e.Button == MouseButtons.Right)
            {
                if (lstTonies.FocusedItem.Bounds.Contains(e.Location))
                {
                    LastSelectediItem = lstTonies.SelectedItems[0];
                    TonieContextMenu.Show(System.Windows.Forms.Cursor.Position);
                }
            }
            else
            {
                if (lstTonies.SelectedItems.Count == 1)
                {
                    if (lstTonies.SelectedItems[0] == LastSelectediItem)
                    {
                        if (lstTonies.SelectedItems[0].ImageKey == "custom")
                        {
                            LastSelectediItem.BeginEdit();
                        }
                    }
                    LastSelectediItem = lstTonies.SelectedItems[0];
                }
            }
        }

        private void lstTonies_DoubleClick(object sender, EventArgs e)
        {
            ReassignSelected();
        }

        private void lstTonies_KeyDown(object sender, KeyEventArgs e)
        {
            if (e.KeyData == Keys.F2 && lstTonies.SelectedItems.Count > 0)
            {
                if (lstTonies.SelectedItems[0].ImageKey == "custom")
                {
                    lstTonies.SelectedItems[0].BeginEdit();
                }
            }
            if (e.Control && e.KeyCode == Keys.A)
            {
                foreach (ListViewItem item in lstTonies.Items)
                {
                    item.Selected = true;
                }
            }
        }

        private void lstTonies_ItemSelectionChanged(object sender, ListViewItemSelectionChangedEventArgs e)
        {
            if (!e.IsSelected)
            {
                LastSelectediItem = null;
            }
            AutoSelected = false;
        }

        private void lstTonies_AfterLabelEdit(object sender, LabelEditEventArgs e)
        {
            ListViewItem item = lstTonies.Items[e.Item];
            ListViewTag tag = item.Tag as ListViewTag;

            if (e.Label == null)
            {
                return;
            }

            lock (TonieInfoLock)
            {
                if (!TonieInfoCustom.ContainsKey(tag.Hash))
                {
                    TonieInfoCustom.Add(tag.Hash, "");
                }
                TonieInfoCustom[tag.Hash] = e.Label;
            }

            SaveJson();
        }

        #endregion


        #region Selected item commands

        private void ReassignSelected()
        {
            if (lstTonies.SelectedItems.Count == 1)
            {
                ListViewItem item = lstTonies.SelectedItems[0];

                ListViewTag tag = item.Tag as ListViewTag;
                var fi = new FileInfo(tag.FileName);
                string oldUid = ReverseUid(fi.Directory.Name + fi.Name);

                AskUIDForm dlg = new AskUIDForm(RfidReader);
                dlg.Uid = oldUid;

                if (dlg.ShowDialog() == DialogResult.OK)
                {
                    string uid = dlg.Uid;
                    string newDir = Path.Combine(CurrentDirectory, ReverseUid(uid).Substring(0, 8));
                    string newFile = Path.Combine(newDir, ReverseUid(uid).Substring(8, 8));

                    if (new FileInfo(newFile).FullName == fi.FullName)
                    {
                        return;
                    }

                    if (new FileInfo(newFile).Exists)
                    {
                        MessageBox.Show("Failed to assign UID '" + uid + "' as this file already exists", "Re-assigning UID failed");
                        return;
                    }

                    try
                    {
                        Directory.CreateDirectory(newDir);
                    }
                    catch (Exception ex)
                    {
                        MessageBox.Show("Failed to create directory '" + newDir + "'", "Re-assigning UID failed");
                        return;
                    }

                    try
                    {
                        DirectoryInfo oldDir = fi.Directory;
                        fi.MoveTo(newFile);
                        if (oldDir.EnumerateFiles().Count() != 0 || (oldDir.EnumerateDirectories().Count() != 0))
                        {
                            MessageBox.Show("Success. Will not delete directory '" + oldDir.FullName + "' as it is not empty.", "Successfully re-assigned UID");
                        }
                        else
                        {
                            oldDir.Delete();
                        }
                    }
                    catch (Exception ex)
                    {
                        MessageBox.Show("Failed to write file '" + newFile + "'", "Re-assigning UID failed");
                        return;
                    }

                    RefreshCardContent();
                }
            }
        }

        private void assignDummyUID_Click(object sender, EventArgs e)
        {
            if (lstTonies.SelectedItems.Count == 1)
            {
                ListViewItem item = lstTonies.SelectedItems[0];

                ListViewTag tag = item.Tag as ListViewTag;
                var fi = new FileInfo(tag.FileName);
                string oldUid = ReverseUid(fi.Directory.Name + fi.Name);

                // generate random UID
                Guid myuuid = Guid.NewGuid();
                string myuuidAsString = myuuid.ToString();
                myuuidAsString = myuuidAsString.Replace("-", string.Empty);
                myuuidAsString = myuuidAsString.Substring(0, 10);
                myuuidAsString = "E00403" + myuuidAsString.ToUpper();

                string uid = myuuidAsString;
                string newDir = Path.Combine(CurrentDirectory, ReverseUid(uid).Substring(0, 8));
                string newFile = Path.Combine(newDir, ReverseUid(uid).Substring(8, 8));

                if (new FileInfo(newFile).FullName == fi.FullName)
                {
                    return;
                }

                if (new FileInfo(newFile).Exists)
                {
                    MessageBox.Show("Failed to assign UID '" + uid + "' as this file already exists", "Re-assigning UID failed");
                    return;
                }

                try
                {
                    Directory.CreateDirectory(newDir);
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to create directory '" + newDir + "'", "Re-assigning UID failed");
                    return;
                }

                try
                {
                    DirectoryInfo oldDir = fi.Directory;
                    fi.MoveTo(newFile);
                    if (oldDir.EnumerateFiles().Count() != 0 || (oldDir.EnumerateDirectories().Count() != 0))
                    {
                        MessageBox.Show("Success. Will not delete directory '" + oldDir.FullName + "' as it is not empty.", "Successfully re-assigned UID");
                    }
                    else
                    {
                        oldDir.Delete();
                    }
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to write file '" + newFile + "'", "Re-assigning UID failed");
                    return;
                }

                RefreshCardContent();
            }
        }

        private void DeleteSelected()
        {
            bool deleteAll = false;

            StopAnalyzeThread();

            if (lstTonies.SelectedItems.Count > 1)
            {
                switch (MessageBox.Show("Delete all selected files?", "Delete files?", MessageBoxButtons.YesNoCancel))
                {
                    case DialogResult.Cancel:
                        return;
                    case DialogResult.Yes:
                        deleteAll = true;
                        break;
                    case DialogResult.No:
                        break;
                }
            }

            foreach (ListViewItem item in lstTonies.SelectedItems)
            {
                ListViewTag tag = item.Tag as ListViewTag;
                string current = item.Text;

                if (!deleteAll)
                {
                    if (MessageBox.Show("Delete '" + current + "'?", "Delete files?", MessageBoxButtons.YesNo) == DialogResult.No)
                    {
                        continue;
                    }
                }
                FileInfo info = new FileInfo(tag.FileName);
                try
                {
                    info.Delete();
                    info.Directory.Delete();
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to delete file/directory '" + tag.FileName + "'");
                    return;
                }
            }

            StartAnalyzeThread();
        }

        private async Task SaveSelected(bool saveAsMp3 = false)
        {
            bool createDirs = false;

            switch (MessageBox.Show("Create directories for selected file(s)?", "Save audio content", MessageBoxButtons.YesNoCancel))
            {
                case DialogResult.Cancel:
                    return;
                case DialogResult.Yes:
                    createDirs = true;
                    break;
                case DialogResult.No:
                    break;
            }

            FolderBrowserDialog dlg = CreateExportFolderBrowserDialog();

            if (dlg.ShowDialog() != DialogResult.OK)
            {
                return;
            }

            string outputLocation = dlg.SelectedPath;

            MessageBox.Show("You are about to export audio content of the tonie files on your SD card. " +
                "Please *do not* share these files as they could contain information which can be used to identify your tonie ID. " +
                "Also be aware that sharing these files is most likely illegal in your country.", "Legal information", MessageBoxButtons.OK);

            List<ListViewTag> selected = lstTonies.SelectedItems.Cast<ListViewItem>()
                .Select(i => i.Tag as ListViewTag)
                .Where(t => t != null)
                .ToList();

            ExportProgressDialog progressDlg = new ExportProgressDialog();
            progressDlg.Text = saveAsMp3 ? "Exporting to .mp3..." : "Exporting to .ogg...";
            progressDlg.SetTotal(selected.Count);
            progressDlg.Show(this);

            bool cancelled = false;
            int finishedCount = 0;

            await Task.Run(() =>
            {
                for (int index = 0; index < selected.Count; index++)
                {
                    if (progressDlg.CancelRequested)
                    {
                        cancelled = true;
                        break;
                    }

                    ListViewTag tag = selected[index];
                    int itemIndex = index;

                    try
                    {
                        string file = tag.FileName;

                        TonieAudio dump = TonieAudio.FromFile(file);

                        string[] titles = null;
                        List<string> tags = new List<string>();
                        tags.Add("TeddyVersion=" + GetVersion());
                        tags.Add("TeddyFile=" + file);

                        string hashString = BitConverter.ToString(dump.Header.Hash).Replace("-", "");
                        IEnumerable<TonieTools.TonieData> found = new TonieTools.TonieData[0];

                        lock (TonieInfoLock)
                        {
                            found = TonieInfo.Where(t => t.Hash.Contains(hashString));
                        }
                        TonieTools.TonieData info = null;

                        if (found.Count() > 0)
                        {
                            info = found.First();
                            titles = info.Tracks;
                            tags.Add("ALBUM=" + info.Title);
                            tags.Add("ARTIST=" + info.Series);
                            tags.Add("LANGUAGE=" + info.Language);
                        }
                        tags.Add("HASH=" + hashString);

                        string displayName = info != null ? info.Title : Path.GetFileName(tag.FileName);

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetCurrentItem(displayName);
                            progressDlg.SetDetail("Reading tonie file...");
                        }));

                        string inFile = new FileInfo(file).Name;
                        string inDir = new FileInfo(file).DirectoryName;
                        string outDirectory = !string.IsNullOrEmpty(outputLocation) ? outputLocation : inDir;

                        if (createDirs)
                        {
                            if (info == null)
                            {
                                outDirectory = Path.Combine(outDirectory, dump.Header.AudioId.ToString("X8") + " - " + tag.Uid);
                            }
                            else
                            {
                                outDirectory = Path.Combine(outDirectory, RemoveInvalidChars(info.Title).Trim());
                            }
                            if (!Directory.Exists(outDirectory))
                            {
                                Directory.CreateDirectory(outDirectory);
                            }
                        }

                        if (!Directory.Exists(outDirectory))
                        {
                            Console.WriteLine("Error: Output directory '" + outDirectory + "' does not exist");
                            return;
                        }

                        // save the tonies.json cover art into the output directory alongside the audio
                        string coverResult;

                        if (info != null && !string.IsNullOrEmpty(info.Pic))
                        {
                            coverResult = SaveCoverImage(info.Pic, hashString, outDirectory);
                        }
                        else
                        {
                            coverResult = info == null
                                ? "skipped (tonie not recognized, no cover URL known)"
                                : "skipped (no cover URL in tonies.json for this tonie)";
                        }

                        // logged at Warning level (rather than Information) so it shows up in the
                        // debug window without the user having to lower the log verbosity first
                        if (coverResult != null)
                        {
                            LogWindow.Log(LogWindow.eLogLevel.Warning, "[SaveSelected] Cover art for '" + displayName + "': " + coverResult);

                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Cover art: " + coverResult);
                            }));
                        }

                        // if this tonie was already exported as MP3 before, skip it and move on to the next one
                        if (saveAsMp3 && IsAlreadyExportedAsMp3(outDirectory))
                        {
                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Skipping - .mp3 files already exist");
                            }));
                            continue;
                        }

                        try
                        {
                            dump.DumpAudioFiles(outDirectory, "", false, tags.ToArray(), titles);
                        }
                        catch (Exception ex)
                        {
                            Console.WriteLine("[ERROR] Failed to write .ogg/.cue'");
                            Console.WriteLine("   Message:    " + ex.Message);
                        }

                        // now try to convert all ogg files to MP3
                        if (saveAsMp3)
                        {
                            // list all ogg files in out directory
                            string[] fileEntries = Directory.GetFiles(outDirectory, "*.ogg");
                            int trackCount = dump.Header.AudioChapters.Length;
                            string coverFileName = FindCoverFile(outDirectory);

                            // loop over all files
                            for (int fileIndex = 0; fileIndex < fileEntries.Length; fileIndex++)
                            {
                                if (progressDlg.CancelRequested)
                                {
                                    cancelled = true;
                                    break;
                                }

                                string fileName = fileEntries[fileIndex];
                                string destinationFileName = outDirectory + "\\" + Path.GetFileNameWithoutExtension(fileName) + ".mp3";

                                progressDlg.Invoke(new Action(() =>
                                {
                                    progressDlg.SetDetail("Converting '" + Path.GetFileName(fileName) + "' to .mp3 (" + (fileIndex + 1) + "/" + fileEntries.Length + ")...");
                                }));

                                bool conversionOk = IsNonEmptyFile(destinationFileName) || ConvertOggToMp3WithFfmpeg(fileName, destinationFileName, info, trackCount, coverFileName);

                                // only delete the ogg once the mp3 exists, otherwise the track would be lost
                                if (conversionOk)
                                {
                                    System.IO.File.Delete(fileName);
                                }
                            }
                        }
                    }
                    catch (Exception ex)
                    {
                        progressDlg.Invoke(new Action(() =>
                        {
                            MessageBox.Show(progressDlg, "Failed to save file '" + tag.FileName + "': " + ex.Message);
                        }));
                    }
                    finally
                    {
                        finishedCount++;

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetFinished(finishedCount);
                        }));
                    }

                    Console.WriteLine("Export finished: ");
                }
            });

            progressDlg.Close();

            MessageBox.Show(cancelled ? "Export cancelled." : "Export finished.", "", MessageBoxButtons.OK);
        }

        /// <summary>
        /// Independent third export option: identical to SaveSelected(true) (same dump/cover/skip
        /// logic), but converts the ogg tracks of a tonie to mp3 in parallel (bounded to the CPU
        /// core count) instead of spawning ffmpeg.exe once per track sequentially. Kept as a fully
        /// separate method on purpose, so the original "Save as .ogg..." and "Save as .mp3..."
        /// paths above are not touched by this.
        /// </summary>
        private async Task SaveSelectedFastMp3()
        {
            bool createDirs = false;

            switch (MessageBox.Show("Create directories for selected file(s)?", "Save audio content", MessageBoxButtons.YesNoCancel))
            {
                case DialogResult.Cancel:
                    return;
                case DialogResult.Yes:
                    createDirs = true;
                    break;
                case DialogResult.No:
                    break;
            }

            FolderBrowserDialog dlg = CreateExportFolderBrowserDialog();

            if (dlg.ShowDialog() != DialogResult.OK)
            {
                return;
            }

            string outputLocation = dlg.SelectedPath;

            MessageBox.Show("You are about to export audio content of the tonie files on your SD card. " +
                "Please *do not* share these files as they could contain information which can be used to identify your tonie ID. " +
                "Also be aware that sharing these files is most likely illegal in your country.", "Legal information", MessageBoxButtons.OK);

            List<ListViewTag> selected = lstTonies.SelectedItems.Cast<ListViewItem>()
                .Select(i => i.Tag as ListViewTag)
                .Where(t => t != null)
                .ToList();

            ExportProgressDialog progressDlg = new ExportProgressDialog();
            progressDlg.Text = "Exporting to .mp3 (parallel)...";
            progressDlg.SetTotal(selected.Count);
            progressDlg.Show(this);

            bool cancelled = false;
            int finishedCount = 0;

            await Task.Run(() =>
            {
                for (int index = 0; index < selected.Count; index++)
                {
                    if (progressDlg.CancelRequested)
                    {
                        cancelled = true;
                        break;
                    }

                    ListViewTag tag = selected[index];
                    int itemIndex = index;

                    try
                    {
                        string file = tag.FileName;

                        TonieAudio dump = TonieAudio.FromFile(file);

                        string[] titles = null;
                        List<string> tags = new List<string>();
                        tags.Add("TeddyVersion=" + GetVersion());
                        tags.Add("TeddyFile=" + file);

                        string hashString = BitConverter.ToString(dump.Header.Hash).Replace("-", "");
                        var found = TonieInfo.Where(t => t.Hash.Contains(hashString));
                        TonieTools.TonieData info = null;

                        if (found.Count() > 0)
                        {
                            info = found.First();
                            titles = info.Tracks;
                            tags.Add("ALBUM=" + info.Title);
                            tags.Add("ARTIST=" + info.Series);
                            tags.Add("LANGUAGE=" + info.Language);
                        }
                        tags.Add("HASH=" + hashString);

                        string displayName = info != null ? info.Title : Path.GetFileName(tag.FileName);

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetCurrentItem(displayName);
                            progressDlg.SetDetail("Reading tonie file...");
                        }));

                        string inFile = new FileInfo(file).Name;
                        string inDir = new FileInfo(file).DirectoryName;
                        string outDirectory = !string.IsNullOrEmpty(outputLocation) ? outputLocation : inDir;

                        if (createDirs)
                        {
                            if (info == null)
                            {
                                outDirectory = Path.Combine(outDirectory, dump.Header.AudioId.ToString("X8") + " - " + tag.Uid);
                            }
                            else
                            {
                                outDirectory = Path.Combine(outDirectory, RemoveInvalidChars(info.Title).Trim());
                            }
                            if (!Directory.Exists(outDirectory))
                            {
                                Directory.CreateDirectory(outDirectory);
                            }
                        }

                        if (!Directory.Exists(outDirectory))
                        {
                            Console.WriteLine("Error: Output directory '" + outDirectory + "' does not exist");
                            return;
                        }

                        // save the tonies.json cover art into the output directory alongside the audio
                        string coverResult;

                        if (info != null && !string.IsNullOrEmpty(info.Pic))
                        {
                            coverResult = SaveCoverImage(info.Pic, hashString, outDirectory);
                        }
                        else
                        {
                            coverResult = info == null
                                ? "skipped (tonie not recognized, no cover URL known)"
                                : "skipped (no cover URL in tonies.json for this tonie)";
                        }

                        if (coverResult != null)
                        {
                            LogWindow.Log(LogWindow.eLogLevel.Warning, "[SaveSelectedFastMp3] Cover art for '" + displayName + "': " + coverResult);

                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Cover art: " + coverResult);
                            }));
                        }

                        // if this tonie was already exported as MP3 before, skip it and move on to the next one
                        if (IsAlreadyExportedAsMp3(outDirectory))
                        {
                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Skipping - .mp3 files already exist");
                            }));
                            continue;
                        }

                        try
                        {
                            dump.DumpAudioFiles(outDirectory, "", false, tags.ToArray(), titles);
                        }
                        catch (Exception ex)
                        {
                            Console.WriteLine("[ERROR] Failed to write .ogg/.cue'");
                            Console.WriteLine("   Message:    " + ex.Message);
                        }

                        // convert all ogg files of this tonie to mp3 in parallel (bounded to the CPU
                        // core count) instead of spawning ffmpeg.exe sequentially, one track at a time
                        string[] fileEntries = Directory.GetFiles(outDirectory, "*.ogg");
                        int trackCount = dump.Header.AudioChapters.Length;
                        string coverFileName = FindCoverFile(outDirectory);

                        if (fileEntries.Length > 0)
                        {
                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Converting " + fileEntries.Length + " track(s) to .mp3 in parallel...");
                            }));

                            int convertedCount = 0;
                            int degreeOfParallelism = Math.Max(1, Environment.ProcessorCount);

                            Parallel.ForEach(fileEntries, new ParallelOptions { MaxDegreeOfParallelism = degreeOfParallelism }, (fileName, loopState) =>
                            {
                                if (progressDlg.CancelRequested)
                                {
                                    loopState.Stop();
                                    return;
                                }

                                string destinationFileName = outDirectory + "\\" + Path.GetFileNameWithoutExtension(fileName) + ".mp3";

                                bool conversionOk = IsNonEmptyFile(destinationFileName) || ConvertOggToMp3WithFfmpeg(fileName, destinationFileName, info, trackCount, coverFileName);

                                // only delete the ogg once the mp3 exists, otherwise the track would be lost
                                if (conversionOk)
                                {
                                    System.IO.File.Delete(fileName);
                                }

                                int done = Interlocked.Increment(ref convertedCount);

                                progressDlg.Invoke(new Action(() =>
                                {
                                    progressDlg.SetDetail("Converted " + done + " of " + fileEntries.Length + " track(s) to .mp3...");
                                }));
                            });

                            if (progressDlg.CancelRequested)
                            {
                                cancelled = true;
                            }
                        }
                    }
                    catch (Exception ex)
                    {
                        progressDlg.Invoke(new Action(() =>
                        {
                            MessageBox.Show(progressDlg, "Failed to save file '" + tag.FileName + "': " + ex.Message);
                        }));
                    }
                    finally
                    {
                        finishedCount++;

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetFinished(finishedCount);
                        }));
                    }

                    Console.WriteLine("Export finished: ");
                }
            });

            progressDlg.Close();

            MessageBox.Show(cancelled ? "Export cancelled." : "Export finished.", "", MessageBoxButtons.OK);
        }

        /// <summary>
        /// Independent fourth export option: same dump/cover/skip logic as the other export
        /// methods, but converts ogg tracks to mp3 fully in-process (Concentus Opus decode via the
        /// existing OpusWaveStream, straight into a NAudio.Lame encoder) instead of spawning
        /// ffmpeg.exe at all. No external process, no 115MB ffmpeg.exe dependency for this path.
        /// Tracks are still converted in parallel (bounded to the CPU core count). Kept as a fully
        /// separate method on purpose - the ffmpeg-based export paths above are not touched.
        /// </summary>
        private async Task SaveSelectedNativeMp3()
        {
            bool createDirs = false;

            switch (MessageBox.Show("Create directories for selected file(s)?", "Save audio content", MessageBoxButtons.YesNoCancel))
            {
                case DialogResult.Cancel:
                    return;
                case DialogResult.Yes:
                    createDirs = true;
                    break;
                case DialogResult.No:
                    break;
            }

            FolderBrowserDialog dlg = CreateExportFolderBrowserDialog();

            if (dlg.ShowDialog() != DialogResult.OK)
            {
                return;
            }

            string outputLocation = dlg.SelectedPath;

            MessageBox.Show("You are about to export audio content of the tonie files on your SD card. " +
                "Please *do not* share these files as they could contain information which can be used to identify your tonie ID. " +
                "Also be aware that sharing these files is most likely illegal in your country.", "Legal information", MessageBoxButtons.OK);

            List<ListViewTag> selected = lstTonies.SelectedItems.Cast<ListViewItem>()
                .Select(i => i.Tag as ListViewTag)
                .Where(t => t != null)
                .ToList();

            ExportProgressDialog progressDlg = new ExportProgressDialog();
            progressDlg.Text = "Exporting to .mp3 (native)...";
            progressDlg.SetTotal(selected.Count);
            progressDlg.Show(this);

            bool cancelled = false;
            int finishedCount = 0;

            await Task.Run(() =>
            {
                for (int index = 0; index < selected.Count; index++)
                {
                    if (progressDlg.CancelRequested)
                    {
                        cancelled = true;
                        break;
                    }

                    ListViewTag tag = selected[index];
                    int itemIndex = index;

                    try
                    {
                        string file = tag.FileName;

                        TonieAudio dump = TonieAudio.FromFile(file, false);

                        string[] titles = null;
                        List<string> tags = new List<string>();
                        tags.Add("TeddyVersion=" + GetVersion());
                        tags.Add("TeddyFile=" + file);

                        string hashString = BitConverter.ToString(dump.Header.Hash).Replace("-", "");
                        var found = TonieInfo.Where(t => t.Hash.Contains(hashString));
                        TonieTools.TonieData info = null;

                        if (found.Count() > 0)
                        {
                            info = found.First();
                            titles = info.Tracks;
                            tags.Add("ALBUM=" + info.Title);
                            tags.Add("ARTIST=" + info.Series);
                            tags.Add("LANGUAGE=" + info.Language);
                        }
                        tags.Add("HASH=" + hashString);

                        string displayName = info != null ? info.Title : Path.GetFileName(tag.FileName);

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetCurrentItem(displayName);
                            progressDlg.SetDetail("Reading tonie file...");
                        }));

                        string inFile = new FileInfo(file).Name;
                        string inDir = new FileInfo(file).DirectoryName;
                        string outDirectory = !string.IsNullOrEmpty(outputLocation) ? outputLocation : inDir;

                        if (createDirs)
                        {
                            if (info == null)
                            {
                                outDirectory = Path.Combine(outDirectory, dump.Header.AudioId.ToString("X8") + " - " + tag.Uid);
                            }
                            else
                            {
                                outDirectory = Path.Combine(outDirectory, RemoveInvalidChars(info.Title).Trim());
                            }
                            if (!Directory.Exists(outDirectory))
                            {
                                Directory.CreateDirectory(outDirectory);
                            }
                        }

                        if (!Directory.Exists(outDirectory))
                        {
                            Console.WriteLine("Error: Output directory '" + outDirectory + "' does not exist");
                            return;
                        }

                        // save the tonies.json cover art into the output directory alongside the audio
                        string coverResult;

                        if (info != null && !string.IsNullOrEmpty(info.Pic))
                        {
                            coverResult = SaveCoverImage(info.Pic, hashString, outDirectory);
                        }
                        else
                        {
                            coverResult = info == null
                                ? "skipped (tonie not recognized, no cover URL known)"
                                : "skipped (no cover URL in tonies.json for this tonie)";
                        }

                        if (coverResult != null)
                        {
                            LogWindow.Log(LogWindow.eLogLevel.Warning, "[SaveSelectedNativeMp3] Cover art for '" + displayName + "': " + coverResult);

                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Cover art: " + coverResult);
                            }));
                        }

                        // if this tonie was already exported as MP3 before, skip it and move on to the next one
                        if (IsAlreadyExportedAsMp3(outDirectory))
                        {
                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Skipping - .mp3 files already exist");
                            }));
                            continue;
                        }

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetDetail("Reading tonie file...");
                        }));

                        dump = TonieAudio.FromFile(file);

                        try
                        {
                            dump.DumpAudioFiles(outDirectory, "", false, tags.ToArray(), titles);
                        }
                        catch (Exception ex)
                        {
                            Console.WriteLine("[ERROR] Failed to write .ogg/.cue'");
                            Console.WriteLine("   Message:    " + ex.Message);
                        }

                        // convert all ogg files of this tonie to mp3 fully in-process: decode the
                        // standalone per-track ogg (written above by DumpAudioFiles, same as the
                        // other export paths) via the existing Opus decoder used for playback, and
                        // encode straight to mp3 with NAudio.Lame - no ffmpeg process involved.
                        string[] fileEntries = Directory.GetFiles(outDirectory, "*.ogg");
                        int trackCount = dump.Header.AudioChapters.Length;
                        byte[] coverArt = null;

                        try
                        {
                            string coverFileName = FindCoverFile(outDirectory);

                            if (coverFileName != null)
                            {
                                coverArt = File.ReadAllBytes(coverFileName);
                            }
                        }
                        catch (Exception ex)
                        {
                            LogWindow.Log(LogWindow.eLogLevel.Warning, "[SaveSelectedNativeMp3] Failed to read cover for '" + displayName + "', exporting without embedded cover: " + ex.Message);
                        }

                        if (fileEntries.Length > 0)
                        {
                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Converting " + fileEntries.Length + " track(s) to .mp3 (native, parallel)...");
                            }));

                            int convertedCount = 0;
                            int degreeOfParallelism = Math.Max(1, Environment.ProcessorCount);

                            Parallel.ForEach(fileEntries, new ParallelOptions { MaxDegreeOfParallelism = degreeOfParallelism }, (fileName, loopState) =>
                            {
                                if (progressDlg.CancelRequested)
                                {
                                    loopState.Stop();
                                    return;
                                }

                                string destinationFileName = outDirectory + "\\" + Path.GetFileNameWithoutExtension(fileName) + ".mp3";

                                // treat a 0-byte mp3 (e.g. left behind by a native-DLL load failure
                                // interrupting LameMP3FileWriter mid-construction) as "not converted"
                                bool conversionOk = System.IO.File.Exists(destinationFileName) && new FileInfo(destinationFileName).Length > 0;

                                if (!conversionOk)
                                {
                                    try
                                    {
                                        int trackNumber = GetTrackNumber(fileName);
                                        ID3TagData id3Tag = new ID3TagData
                                        {
                                            Title = (titles != null && trackNumber > 0 && trackNumber <= titles.Length) ? titles[trackNumber - 1] : Path.GetFileNameWithoutExtension(fileName),
                                            Track = trackNumber > 0 ? trackNumber + "/" + trackCount : null,
                                            AlbumArt = coverArt
                                        };

                                        if (info != null)
                                        {
                                            id3Tag.Artist = info.Series;
                                            id3Tag.Album = !string.IsNullOrEmpty(info.Episodes) ? info.Episodes : info.Title;
                                        }

                                        using (FileStream inputStream = File.OpenRead(fileName))
                                        using (OpusWaveStream opusStream = new OpusWaveStream(inputStream, 48000, 2))
                                        using (LameMP3FileWriter mp3Writer = new LameMP3FileWriter(destinationFileName, opusStream.WaveFormat, LAMEPreset.STANDARD, id3Tag))
                                        {
                                            opusStream.CopyTo(mp3Writer);
                                        }

                                        conversionOk = System.IO.File.Exists(destinationFileName) && new FileInfo(destinationFileName).Length > 0;
                                    }
                                    catch (Exception ex)
                                    {
                                        LogWindow.Log(LogWindow.eLogLevel.Warning, "[SaveSelectedNativeMp3] Failed to convert '" + fileName + "' to mp3: " + ex.Message);

                                        // don't leave a partial/corrupt mp3 behind on failure
                                        try { System.IO.File.Delete(destinationFileName); } catch { }
                                        conversionOk = false;
                                    }
                                }

                                if (conversionOk)
                                {
                                    // only remove the source ogg once the mp3 is confirmed written -
                                    // never delete it on a failed conversion, or the track is lost entirely
                                    System.IO.File.Delete(fileName);
                                }
                                else
                                {
                                    LogWindow.Log(LogWindow.eLogLevel.Warning, "[SaveSelectedNativeMp3] Keeping '" + fileName + "' - mp3 conversion did not succeed, can be retried later.");
                                }

                                int done = Interlocked.Increment(ref convertedCount);

                                progressDlg.Invoke(new Action(() =>
                                {
                                    progressDlg.SetDetail("Converted " + done + " of " + fileEntries.Length + " track(s) to .mp3 (native)...");
                                }));
                            });

                            if (progressDlg.CancelRequested)
                            {
                                cancelled = true;
                            }
                        }
                    }
                    catch (Exception ex)
                    {
                        progressDlg.Invoke(new Action(() =>
                        {
                            MessageBox.Show(progressDlg, "Failed to save file '" + tag.FileName + "': " + ex.Message);
                        }));
                    }
                    finally
                    {
                        finishedCount++;

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetFinished(finishedCount);
                        }));
                    }

                    Console.WriteLine("Export finished: ");
                }
            });

            progressDlg.Close();

            MessageBox.Show(cancelled ? "Export cancelled." : "Export finished.", "", MessageBoxButtons.OK);
        }

        /// <summary>
        /// Standalone maintenance action: (re-)fetches the cover art for already-exported tonies
        /// into their existing export directories, without touching or re-exporting any audio.
        /// Locates each tonie's export folder using the same "subdirectory per tonie?" / output
        /// location logic as the export methods above, and skips any tonie whose folder doesn't
        /// exist yet (nothing was exported there). Unlike the export paths - which leave an
        /// existing cover.&lt;ext&gt; alone to avoid needless re-downloads on a resumed batch - this
        /// always replaces it, since the whole point of this button is to refresh/fix covers.
        /// </summary>
        private async Task UpdateCoversOnly()
        {
            bool createDirs = false;

            switch (MessageBox.Show("Were per-tonie subdirectories created when you exported?", "Update cover art", MessageBoxButtons.YesNoCancel))
            {
                case DialogResult.Cancel:
                    return;
                case DialogResult.Yes:
                    createDirs = true;
                    break;
                case DialogResult.No:
                    break;
            }

            FolderBrowserDialog dlg = CreateExportFolderBrowserDialog();

            if (dlg.ShowDialog() != DialogResult.OK)
            {
                return;
            }

            string outputLocation = dlg.SelectedPath;

            List<ListViewTag> selected = lstTonies.SelectedItems.Cast<ListViewItem>()
                .Select(i => i.Tag as ListViewTag)
                .Where(t => t != null)
                .ToList();

            ExportProgressDialog progressDlg = new ExportProgressDialog();
            progressDlg.Text = "Updating cover art...";
            progressDlg.SetTotal(selected.Count);
            progressDlg.Show(this);

            bool cancelled = false;
            int finishedCount = 0;

            await Task.Run(() =>
            {
                for (int index = 0; index < selected.Count; index++)
                {
                    if (progressDlg.CancelRequested)
                    {
                        cancelled = true;
                        break;
                    }

                    ListViewTag tag = selected[index];

                    try
                    {
                        string file = tag.FileName;

                        TonieAudio dump = TonieAudio.FromFile(file);

                        string hashString = BitConverter.ToString(dump.Header.Hash).Replace("-", "");
                        var found = TonieInfo.Where(t => t.Hash.Contains(hashString));
                        TonieTools.TonieData info = found.Count() > 0 ? found.First() : null;

                        string displayName = info != null ? info.Title : Path.GetFileName(tag.FileName);

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetCurrentItem(displayName);
                            progressDlg.SetDetail("Locating export directory...");
                        }));

                        string inDir = new FileInfo(file).DirectoryName;
                        string outDirectory = !string.IsNullOrEmpty(outputLocation) ? outputLocation : inDir;

                        if (createDirs)
                        {
                            outDirectory = Path.Combine(outDirectory, info == null
                                ? dump.Header.AudioId.ToString("X8") + " - " + tag.Uid
                                : RemoveInvalidChars(info.Title).Trim());
                        }

                        if (!Directory.Exists(outDirectory))
                        {
                            progressDlg.Invoke(new Action(() =>
                            {
                                progressDlg.SetDetail("Skipping - export directory not found (" + outDirectory + ")");
                            }));
                            continue;
                        }

                        string coverResult;

                        if (info != null && !string.IsNullOrEmpty(info.Pic))
                        {
                            // force-replace, even if a cover already exists in this directory
                            coverResult = SaveCoverImage(info.Pic, hashString, outDirectory, overwrite: true);
                        }
                        else
                        {
                            coverResult = info == null
                                ? "skipped (tonie not recognized, no cover URL known)"
                                : "skipped (no cover URL in tonies.json for this tonie)";
                        }

                        LogWindow.Log(LogWindow.eLogLevel.Warning, "[UpdateCoversOnly] Cover art for '" + displayName + "': " + coverResult);

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetDetail("Cover art: " + coverResult);
                        }));
                    }
                    catch (Exception ex)
                    {
                        progressDlg.Invoke(new Action(() =>
                        {
                            MessageBox.Show(progressDlg, "Failed to update cover for '" + tag.FileName + "': " + ex.Message);
                        }));
                    }
                    finally
                    {
                        finishedCount++;

                        progressDlg.Invoke(new Action(() =>
                        {
                            progressDlg.SetFinished(finishedCount);
                        }));
                    }
                }
            });

            progressDlg.Close();

            MessageBox.Show(cancelled ? "Cover update cancelled." : "Cover update finished.", "", MessageBoxButtons.OK);
        }

        private void ExportSelected()
        {
            FolderBrowserDialog dlg = CreateExportFolderBrowserDialog();

            if (dlg.ShowDialog() != DialogResult.OK)
            {
                return;
            }
            string outputLocation = dlg.SelectedPath;

            foreach (ListViewItem item in lstTonies.SelectedItems)
            {
                ListViewTag tag = item.Tag as ListViewTag;
                string current = item.Text;

                try
                {
                    string destName = Path.Combine(outputLocation, tag.Info.Model + " - " + tag.AudioId.ToString("X8") + " - " + RemoveInvalidChars(tag.Info.Title).Trim());
                    File.Copy(tag.FileName, destName, false);

                }
                catch (Exception ex)
                {
                    MessageBox.Show("Failed to save file '" + tag.FileName + "': " + ex.Message);
                    return;
                }
            }
        }

        /// <summary>
        /// A tonie counts as exported once its directory has non-empty mp3s and no ogg is left
        /// unconverted - so a partly failed export is picked up again instead of being skipped.
        /// </summary>
        private static bool IsAlreadyExportedAsMp3(string directory)
        {
            return Directory.GetFiles(directory, "*.mp3").Any(IsNonEmptyFile) && Directory.GetFiles(directory, "*.ogg").Length == 0;
        }

        /// <summary>
        /// DumpAudioFiles names the tracks "Track_01.ogg", "Track_02.ogg", ... - returns 0 if the
        /// number can't be determined.
        /// </summary>
        private static int GetTrackNumber(string fileName)
        {
            Match match = Regex.Match(Path.GetFileNameWithoutExtension(fileName), @"Track_(\d+)$");

            return match.Success ? int.Parse(match.Groups[1].Value) : 0;
        }

        /// <summary>
        /// Quotes one command line argument following the Windows (CommandLineToArgvW) rules, so
        /// values containing quotes - e.g. 'Der kleine König sagt "Gute Nacht"' - or trailing
        /// backslashes reach the child process unchanged.
        /// </summary>
        private static string QuoteArgument(string argument)
        {
            StringBuilder quoted = new StringBuilder("\"");
            int backslashes = 0;

            foreach (char c in argument ?? "")
            {
                if (c == '\\')
                {
                    backslashes++;
                    continue;
                }

                // backslashes are only special in front of a quote
                quoted.Append('\\', c == '"' ? backslashes * 2 + 1 : backslashes);
                quoted.Append(c);
                backslashes = 0;
            }

            quoted.Append('\\', backslashes * 2);
            quoted.Append('"');

            return quoted.ToString();
        }

        /// <summary>
        /// Converts one exported .ogg track to .mp3 with ffmpeg. The tags DumpAudioFiles wrote into
        /// the ogg (track title, ...) are stream-level comments, which ffmpeg only carries over into
        /// the mp3 when mapped explicitly. Adds artist, album and track number and embeds the cover,
        /// if there is one. Returns true only if ffmpeg succeeded and wrote a non-empty mp3; on
        /// failure no partial mp3 is left behind.
        /// </summary>
        private static bool ConvertOggToMp3WithFfmpeg(string oggFileName, string mp3FileName, TonieTools.TonieData info, int trackCount, string coverFileName)
        {
            StringBuilder args = new StringBuilder("-nostdin -y -i " + QuoteArgument(oggFileName));

            if (coverFileName != null)
            {
                args.Append(" -i " + QuoteArgument(coverFileName));
            }

            args.Append(" -map 0:a -map_metadata 0:s:a:0");

            if (coverFileName != null)
            {
                args.Append(" -map 1:v -c:v copy -disposition:v attached_pic -metadata:s:v title=\"Album cover\" -metadata:s:v comment=\"Cover (front)\"");
            }

            // without these, the ALBUM/ARTIST from the ogg are kept
            if (info != null && !string.IsNullOrEmpty(info.Series))
            {
                args.Append(" -metadata artist=" + QuoteArgument(info.Series));
            }
            if (info != null && !string.IsNullOrEmpty(info.Episodes))
            {
                args.Append(" -metadata album=" + QuoteArgument(info.Episodes));
            }

            int trackNumber = GetTrackNumber(oggFileName);

            if (trackNumber > 0)
            {
                args.Append(" -metadata track=" + trackNumber + "/" + trackCount);
            }

            // ID3v2.3 for compatibility - Windows Explorer and many players don't show v2.4 cover art
            args.Append(" -id3v2_version 3 " + QuoteArgument(mp3FileName));

            int exitCode;

            using (Process p = new Process())
            {
                p.StartInfo.UseShellExecute = false;
                p.StartInfo.CreateNoWindow = true;
                p.StartInfo.FileName = FfmpegPath;
                p.StartInfo.Arguments = args.ToString();
                p.Start();
                p.WaitForExit();
                exitCode = p.ExitCode;
            }

            if (exitCode == 0 && IsNonEmptyFile(mp3FileName))
            {
                return true;
            }

            try { File.Delete(mp3FileName); } catch { }

            // an unusable cover image must not cost the audio - retry without it
            if (coverFileName != null)
            {
                LogWindow.Log(LogWindow.eLogLevel.Warning, "[ConvertOggToMp3WithFfmpeg] ffmpeg failed (exit code " + exitCode + ") for '" + oggFileName + "' with cover '" + coverFileName + "', retrying without cover");
                return ConvertOggToMp3WithFfmpeg(oggFileName, mp3FileName, info, trackCount, null);
            }

            LogWindow.Log(LogWindow.eLogLevel.Warning, "[ConvertOggToMp3WithFfmpeg] ffmpeg failed (exit code " + exitCode + ") for '" + oggFileName + "', keeping the .ogg");
            return false;
        }

        #endregion

        private void cmbSorting_SelectedIndexChanged(object sender, EventArgs e)
        {
            UpdateSorting();
        }

        private void UpdateSorting()
        {
            foreach (ListViewItem item in lstTonies.Items)
            {
                if (cmbSorting.SelectedIndex == 3)
                {
                    switch (item.ImageKey)
                    {
                        case "unknown":
                            item.Group = lstTonies.Groups[0];
                            break;
                        case "custom":
                            item.Group = lstTonies.Groups[1];
                            break;
                        default:
                            item.Group = lstTonies.Groups[2];
                            break;
                    }
                }
                else
                {
                    item.Group = lstTonies.Groups[3];
                }
            }
            (lstTonies.ListViewItemSorter as ListViewItemComparer).Characteristic = cmbSorting.SelectedIndex;
            lstTonies.Sort();
        }

        private void exitToolStripMenuItem_Click(object sender, EventArgs e)
        {
            StopThreads();
            Application.Exit();
        }

        private async void btnSave_Click(object sender, EventArgs e)
        {
            await SaveSelected();
        }

        private async void btnSaveMp3_Click(object sender, EventArgs e)
        {
            await SaveSelected(true);
        }

        private async void btnSaveMp3Fast_Click(object sender, EventArgs e)
        {
            await SaveSelectedFastMp3();
        }

        private async void btnSaveMp3Native_Click(object sender, EventArgs e)
        {
            await SaveSelectedNativeMp3();
        }

        private async void btnUpdateCovers_Click(object sender, EventArgs e)
        {
            await UpdateCoversOnly();
        }

        public static string RemoveInvalidChars(string filename)
        {
            return string.Concat(filename.Split(Path.GetInvalidFileNameChars()));
        }

        private string GetVersion()
        {
            return Application.ProductVersion + (ThisAssembly.Git.IsDirty ? ",dirty" : "");
        }

        private async void reportselectedFilesToolStripMenuItem_Click(object sender, EventArgs e)
        {
            await ReportSelected();
        }

        private void toggleLiveFlagToolStripMenuItem_Click(object sender, EventArgs e)
        {
            bool bUnhideAll = (lstTonies.SelectedItems.Count > 1);

            foreach (ListViewItem t in lstTonies.SelectedItems)
            {
                ListViewTag tag = t.Tag as ListViewTag;

                if (bUnhideAll || tag.FileInfo.Attributes.HasFlag(FileAttributes.Hidden))
                {
                    tag.FileInfo.Attributes &= ~FileAttributes.Hidden;
                }
                else
                {
                    tag.FileInfo.Attributes |= FileAttributes.Hidden;
                }
            }

            RefreshCardContent();
        }

        private async Task<bool> ReportSelected()
        {
            if (lstTonies.SelectedItems.Count == 0)
            {
                return false;
            }

            StringBuilder str = new StringBuilder();

            str.AppendLine(" Reporting " + lstTonies.SelectedItems.Count + " files");
            str.AppendLine("-----------------------------------");

            foreach (ListViewItem t in lstTonies.SelectedItems)
            {
                ListViewTag tag = t.Tag as ListViewTag;

                AddInfo(str, tag);
            }

            ReportForm form = new ReportForm(str.ToString());

            if (form.ShowDialog() == DialogResult.OK)
            {
                Settings.Username = form.Username;
                SaveSettings();

                DiagStatus ret = await DiagnosticsSendInfo(str.ToString(), form.Username, form.Message, "Audio Report.txt");
                if (ret.Success)
                {
                    MessageBox.Show("Report Successfully sent." + Environment.NewLine + "Server response:" + Environment.NewLine + ret.Message + "", "Report sent", MessageBoxButtons.OK, MessageBoxIcon.Information);
                    return true;
                }
                else
                {
                    MessageBox.Show("Error sending the report." + Environment.NewLine + "Server response:" + Environment.NewLine + ret.Message + "", "Report sent", MessageBoxButtons.OK, MessageBoxIcon.Error);
                }
            }

            return false;
        }


        private async void reportNFCTagToolStripMenuItem_Click(object sender, EventArgs e)
        {
            if (RfidReader == null || AsyncTagActionThread != null)
            {
                return;
            }

            AsyncTagActionThread = new SafeThread(() =>
            {
                try
                {
                    byte[] data = RfidReader.ReadMemory();

                    Invoke(new Action(async () =>
                    {

                        if (data != null)
                        {
                            StringBuilder str = new StringBuilder();

                            str.AppendLine(" Reporting NFC tag content");
                            str.AppendLine("-----------------------------------");
                            str.AppendLine("UUID:        " + BitConverter.ToString(data.Take(8).ToArray()).Replace("-", ""));
                            str.AppendLine("Memory:      " + BitConverter.ToString(data.Skip(8).ToArray()).Replace("-", ""));

                            ReportForm form = new ReportForm(str.ToString());

                            if (form.ShowDialog() == DialogResult.OK)
                            {
                                Settings.Username = form.Username;
                                SaveSettings();

                                DiagStatus ret= await DiagnosticsSendInfo(str.ToString(), form.Username, form.Message, "Tag Dump.txt");
                                if (ret.Success)
                                {
                                    MessageBox.Show("Report Successfully sent." + Environment.NewLine + "Server response:" + Environment.NewLine + ret.Message + "", "Report sent", MessageBoxButtons.OK, MessageBoxIcon.Information);
                                }
                                else
                                {
                                    MessageBox.Show("Error sending the report." + Environment.NewLine + "Server response:" + Environment.NewLine + ret.Message + "", "Report sent", MessageBoxButtons.OK, MessageBoxIcon.Error);
                                }

                            }
                        }
                        else
                        {
                            MessageBox.Show("No tag found. Please make sure you position it correctly.", "Failed");
                        }
                    }));
                }
                catch (Exception ex)
                {
                }
                AsyncTagActionThread = null;
            }, "AsyncTagActionThread");
            AsyncTagActionThread.Start();
            TagOperationDialog opDlg = new TagOperationDialog(true);

            opDlg.Show();

            await Task.Run(() =>
            {
                while (AsyncTagActionThread != null)
                {
                    Thread.Sleep(100);
                    if (opDlg.DialogResult == DialogResult.Cancel)
                    {
                        AsyncTagActionThread.Abort();
                        AsyncTagActionThread = null;
                    }
                }
                Invoke(new Action(() => opDlg.Close()));
            });
        }

        private async void reportProxmarkAnToolStripMenuItem_Click(object sender, EventArgs e)
        {
            TagOperationDialog opDlg = new TagOperationDialog();

            opDlg.Show();
            Proxmark3.MeasurementResult result = RfidReader.MeasureAntenna();

            opDlg.Close();

            if (result == null)
            {
                MessageBox.Show("Measurement failed.", "Failed");
                return;
            }

            string content = " Reporting Proxmark3 antenna performance" + Environment.NewLine;
            content += "------------------------------------------" + Environment.NewLine;

            content += "HF:     " + result.vHF.ToString("0.00", CultureInfo.InvariantCulture) + " V" + Environment.NewLine;
            content += "LF125:  " + result.vLF125.ToString("0.00", CultureInfo.InvariantCulture) + " V" + Environment.NewLine;
            content += "LF134:  " + result.vLF134.ToString("0.00", CultureInfo.InvariantCulture) + " V" + Environment.NewLine;
            content += "LFfopt: " + (result.GetPeakFrequency() / 1000.0f).ToString("0.00", CultureInfo.InvariantCulture) + " kHz" + Environment.NewLine;
            content += "LFVopt: " + result.peakV.ToString("0.00", CultureInfo.InvariantCulture) + " V" + Environment.NewLine;

            ReportForm form = new ReportForm(content);

            if (form.ShowDialog() == DialogResult.OK)
            {
                Settings.Username = form.Username;
                SaveSettings();

                DiagStatus ret = await DiagnosticsSendInfo(content, form.Username, form.Message, "Antenna Report.txt");
                if (ret.Success)
                {
                    MessageBox.Show("Report Successfully sent." + Environment.NewLine + "Server response:" + Environment.NewLine + ret.Message + "", "Report sent", MessageBoxButtons.OK, MessageBoxIcon.Information);
                }
                else
                {
                    MessageBox.Show("Error sending the report." + Environment.NewLine + "Server response:" + Environment.NewLine + ret.Message + "", "Report sent", MessageBoxButtons.OK, MessageBoxIcon.Error);
                }

            }
        }

        public class DiagStatus
        {
            public bool Success;
            public string Message;
        }

        private async Task<DiagStatus> DiagnosticsSendInfo(string payload, string sender, string message, string filename)
        {
            DiagStatus ret = new DiagStatus();

            ret.Success = false;
            ret.Message = "";

            try
            {
                HttpClient httpClient = new HttpClient();
                MultipartFormDataContent form = new MultipartFormDataContent();

                form.Add(new StringContent(sender), "sender");
                form.Add(new StringContent(payload), "payload");
                form.Add(new StringContent(message), "message");
                form.Add(new StringContent(filename), "filename");
                form.Add(new StringContent(GetVersion()), "version");

                HttpResponseMessage response = await httpClient.PostAsync("https://api.revvox.de/diag", form);
                response.EnsureSuccessStatusCode();
                httpClient.Dispose();
                string sd = response.Content.ReadAsStringAsync().Result;

                LogWindow.Log(LogWindow.eLogLevel.Debug, sd);

                ret.Success = (response.StatusCode == HttpStatusCode.OK);
                ret.Message = sd;
                return ret;
            }
            catch (Exception ee)
            {
            }
            return ret;
        }

        private void AddInfo(StringBuilder str, ListViewTag tag)
        {
            lock (TonieInfoLock)
            {
                string customName = null;
                if (TonieInfoCustom.ContainsKey(tag.Hash))
                {
                    customName = TonieInfoCustom[tag.Hash];
                }
                TonieTools.DumpInfo(str, TonieTools.eDumpFormat.FormatText, tag.FileName, TonieInfo, customName);
            }
        }

        private async void readContentToolStripMenuItem_Click(object sender, EventArgs e)
        {
            if (RfidReader == null || AsyncTagActionThread != null)
            {
                return;
            }

            AsyncTagActionThread = new SafeThread(() =>
            {
                try
                {
                    byte[] data = RfidReader.ReadMemory();

                    Invoke(new Action(() =>
                    {
                        if (data != null)
                        {
                            TagDumpDialog dlg = new TagDumpDialog(true, BitConverter.ToString(data).Replace("-", ""));
                            dlg.ShowDialog();
                        }
                        else
                        {
                            MessageBox.Show("No tag found. Please make sure you position it correctly.", "Failed");
                        }
                    }));
                }
                catch (Exception ex)
                {
                }
                AsyncTagActionThread = null;
            }, "AsyncTagActionThread");
            AsyncTagActionThread.Start();
            TagOperationDialog opDlg = new TagOperationDialog(true);

            opDlg.Show();

            await Task.Run(() =>
            {
                while (AsyncTagActionThread != null)
                {
                    Thread.Sleep(100);
                    if (opDlg.DialogResult == DialogResult.Cancel)
                    {
                        AsyncTagActionThread.Abort();
                        AsyncTagActionThread = null;
                    }
                }
                Invoke(new Action(() => opDlg.Close()));
            });
        }

        private async void emulateTagToolStripMenuItem_Click(object sender, EventArgs e)
        {
            if (RfidReader == null || AsyncTagActionThread != null)
            {
                return;
            }

            string previous = "";

            while (true)
            {
                TagDumpDialog dlg = new TagDumpDialog(false, previous);

                if (dlg.ShowDialog() == DialogResult.OK)
                {
                    byte[] data = null;

                    previous = dlg.String;

                    try
                    {
                        data = Helpers.ConvertHexStringToByteArray(previous);
                    }
                    catch (Exception ex)
                    {
                        MessageBox.Show("Failed to parse '" + previous + "'." + Environment.NewLine + "The hex code must represent 40 byte in hexadecimal numbers:" + Environment.NewLine + "8 byte UUID and 32 byte data.");
                        continue;
                    }

                    AsyncTagActionThread = new SafeThread(() =>
                    {
                        try
                        {
                            RfidReader.EmulateTag(data);
                        }
                        catch (Exception ex)
                        {
                        }
                        AsyncTagActionThread = null;
                    }, "AsyncTagActionThread");
                    AsyncTagActionThread.Start();
                }
                break;
            }

            TagOperationDialog opDlg = new TagOperationDialog();

            opDlg.Show();

            await Task.Run(() =>
            {
                while (AsyncTagActionThread != null)
                {
                    Thread.Sleep(100);
                    if (opDlg.DialogResult == DialogResult.Cancel)
                    {
                        AsyncTagActionThread.Abort();
                        AsyncTagActionThread = null;
                    }
                }
                Invoke(new Action(() => opDlg.Close()));
            });
        }

        private void autodetectionEnabledToolStripMenuItem_CheckedChanged(object sender, EventArgs e)
        {
            Settings.NfcEnabled = autodetectionEnabledToolStripMenuItem.Checked;
            SaveSettings();
            UpdateNfcReader();
        }

        private void UpdateNfcReader()
        {
            if (Settings.NfcEnabled)
            {
                if (RfidReader == null)
                {
                    if (Settings.NfcType == "Proxmark3")
                    {
                        RfidReader = new Proxmark3();
                        RfidReader.UidFound += Proxmark3_UidFound;
                        RfidReader.DeviceFound += Proxmark3_DeviceFound;
                        RfidReader.FlashRequest += Proxmark3_FlashRequest;
                        RfidReader.FlashResult += Proxmark3_FlashResult;
                        RfidReader.Start();
                    }
                    else if (Settings.NfcType == "Pn5180Esp")
                    {
                        RfidReader = new Pn5180Esp();
                        RfidReader.UidFound += Proxmark3_UidFound;
                        RfidReader.DeviceFound += Proxmark3_DeviceFound;
                        RfidReader.Start();
                    }
                }
            }
            else
            {
                if (RfidReader != null)
                {
                    RfidReader.Stop();
                    RfidReader = null;
                }
            }
        }

        private void autodetectionEnabledToolStripMenuItem_Click(object sender, EventArgs e)
        {
            autodetectionEnabledToolStripMenuItem.Checked ^= true;
        }

        private void enableDebugModeToolStripMenuItem_Click(object sender, EventArgs e)
        {
            enableDebugModeToolStripMenuItem.Checked ^= true;

            Settings.DebugWindow = enableDebugModeToolStripMenuItem.Checked;
            SaveSettings();

            if (Settings.DebugWindow)
            {
                LogWindow.LogLevel = LogWindow.eLogLevel.DebugVerbose;
                Log.Show();
            }
            else
            {
                LogWindow.LogLevel = LogWindow.eLogLevel.Warning;
                Log.Hide();
            }
        }

        private void measureAnteannaToolStripMenuItem_Click(object sender, EventArgs e)
        {
            TagOperationDialog opDlg = new TagOperationDialog();

            opDlg.Show();
            try
            {
                Proxmark3.MeasurementResult result = RfidReader.MeasureAntenna();

                opDlg.Close();

                if (result == null)
                {
                    MessageBox.Show("Measurement failed.", "Failed");
                    return;
                }

                PlotAntennaForm form = new PlotAntennaForm(result);

                form.ShowDialog();
            }
            catch (Exception ex)
            {
            }
        }

        private void flashFirmwareToolStripMenuItem_Click(object sender, EventArgs e)
        {
            OpenFileDialog dlg = new OpenFileDialog();

            dlg.Title = "Please select the fullimage.elf you want to flash";
            dlg.Filter = "Firmware ELF files (*.elf)|*.elf|All files (*.*)|*.*";
            if (dlg.ShowDialog() == DialogResult.OK)
            {
                try
                {
                    RfidReader.EnterBootloader(dlg.FileName);
                }
                catch (Exception ex)
                {
                }
            }
        }

        private void flashBootloaderToolStripMenuItem_Click(object sender, EventArgs e)
        {
            OpenFileDialog dlg = new OpenFileDialog();

            dlg.Title = "Please select the bootrom.elf you want to flash";
            dlg.Filter = "Bootloader ELF files (*.elf)|*.elf|All files (*.*)|*.*";
            if (dlg.ShowDialog() == DialogResult.OK)
            {
                try
                {
                    RfidReader.EnterBootloader(dlg.FileName);
                }
                catch (Exception ex)
                {
                }
            }
        }

        private void consoleModeToolStripMenuItem_Click(object sender, EventArgs e)
        {
            if (!consoleModeToolStripMenuItem.Checked)
            {
                consoleModeToolStripMenuItem.Checked = true;
                RfidReader.EnterConsole();
            }
            else
            {
                consoleModeToolStripMenuItem.Checked = false;
                RfidReader.ExitConsole();
            }
        }

        private void btnPlay_Click(object sender, EventArgs e)
        {
            if (PlayThread == null)
            {
                if (lstTonies.SelectedItems.Count != 1)
                {
                    return;
                }

                var selected = lstTonies.SelectedItems[0];
                ListViewTag tag = selected.Tag as ListViewTag;
                string file = tag.FileName;

                PlayThread = new SafeThread(() =>
                {
                    try
                    {
                        TonieAudio dump = TonieAudio.FromFile(file);

                        using (var waveOut = new WaveOutEvent())
                        {
                            using (var reader = new OpusWaveStream(new MemoryStream(dump.Audio), 48000, 2))
                            {

                                waveOut.Init(reader);
                                waveOut.PlaybackStopped += WaveOut_PlaybackStopped;
                                waveOut.Play();
                                LogWindow.Log(LogWindow.eLogLevel.Debug, "Playback started");

                                while (!PlayThreadStop)
                                {
                                    Thread.Sleep(100);
                                    int pos = (int)reader.OpusDecoder.CurrentTime.TotalSeconds;
                                    int len = (int)reader.OpusDecoder.TotalTime.TotalSeconds;

                                    string time = reader.OpusDecoder.CurrentTime.ToString(@"hh\:mm\:ss") + "/" + reader.OpusDecoder.TotalTime.ToString(@"hh\:mm\:ss");

                                    if (TrackNewPosition >= 0)
                                    {
                                        TimeSpan newPos = new TimeSpan(0, 0, 0, TrackNewPosition);
                                        reader.SeekTo(newPos);
                                        TrackNewPosition = -1;
                                    }

                                    BeginInvoke(new Action(() =>
                                    {
                                        if (!TrackMouseDown)
                                        {
                                            if (trackPlayPosition.Maximum != len)
                                            {
                                                trackPlayPosition.Maximum = len;
                                                trackPlayPosition.TickFrequency = 60;
                                            }
                                            trackPlayPosition.Value = pos;
                                        }
                                        lblPlayTime.Text = time;
                                    }));
                                }
                                waveOut.Stop();
                            }
                        }
                    }
                    catch (Exception ex)
                    {
                        MessageBox.Show("Failed to play file '" + tag.FileName + "': " + ex.Message);
                    }
                }, "PlayThread");

                btnPlay.Text = "Stop";
                PlayThreadStop = false;
                PlayThread.Start();
            }
            else
            {
                PlayThreadStop = true;
            }
        }

        private void WaveOut_PlaybackStopped(object sender, StoppedEventArgs e)
        {
            LogWindow.Log(LogWindow.eLogLevel.Debug, "Playback stopped");

            BeginInvoke(new Action(() =>
            {
                PlayThreadStop = true;
                btnPlay.Text = "Play file";
                PlayThread = null;
            }));
        }

        private void trackPlayPosition_Scroll(object sender, EventArgs e)
        {
            TrackNewPosition = trackPlayPosition.Value;
        }

        private void trackPlayPosition_MouseDown(object sender, MouseEventArgs e)
        {
            TrackMouseDown = true;
        }

        private void trackPlayPosition_MouseUp(object sender, MouseEventArgs e)
        {
            TrackMouseDown = false;
        }

        internal void ReportException(string name, Exception e)
        {
            if (MessageBox.Show("Sorry, there was an exception in '" + name + "'. Do you want to report that crash?", "Some Exception happened", MessageBoxButtons.YesNo) == DialogResult.No)
            {
                return;
            }

            StringBuilder str = new StringBuilder();

            str.AppendLine(" Reporting an unhandled eception");
            str.AppendLine("-----------------------------------");

            str.AppendLine("Thread: '" + name + "'");
            str.AppendLine("");
            str.AppendLine("-----------------------------------");
            str.AppendLine("");
            str.AppendLine("Message:");
            str.AppendLine(e.Message);
            str.AppendLine("");
            str.AppendLine("-----------------------------------");
            str.AppendLine("");
            str.AppendLine("StackTrace:");
            str.AppendLine(e.StackTrace);
            str.AppendLine("");
            str.AppendLine("-----------------------------------");
            str.AppendLine("");


            ReportForm form = new ReportForm(str.ToString());

            if (form.ShowDialog() == DialogResult.OK)
            {
                Settings.Username = form.Username;
                SaveSettings();

                DiagStatus ret = DiagnosticsSendInfo(str.ToString(), form.Username, form.Message, "Exception.txt").Result;
                if (ret.Success)
                {
                    MessageBox.Show("Report Successfully sent." + Environment.NewLine + "Server response:" + Environment.NewLine + ret.Message + "", "Report sent", MessageBoxButtons.OK, MessageBoxIcon.Information);
                }
                else
                {
                    MessageBox.Show("Error sending the report." + Environment.NewLine + "Server response:" + Environment.NewLine + ret.Message + "", "Report sent", MessageBoxButtons.OK, MessageBoxIcon.Error);
                }

            }
        }

        private void createCustomTagImageToolStripMenuItem_Click(object sender, EventArgs e)
        {
            FolderBrowserDialog dlg = new FolderBrowserDialog();

            if (dlg.ShowDialog() != DialogResult.OK)
            {
                return;
            }
            string outputLocation = dlg.SelectedPath;

            // create a new PDF document
            PdfDocument document = new PdfDocument();

            // Create an empty page
            PdfPage page = document.AddPage();

            // get an XGraphics object for drawing
            XGraphics gfx = XGraphics.FromPdfPage(page, XGraphicsUnit.Millimeter);

            int tagColumn = Settings.CustomTagInitialColumn;
            int tagRow = Settings.CustomTagInitialRow;
            int offsetIncrementX = Settings.CustomTagSize + Settings.CustomTagGapX;
            int offsetIncrementY = Settings.CustomTagSize + Settings.CustomTagGapY;

            // loop over all selected objects
            foreach (ListViewItem item in lstTonies.SelectedItems)
            {
                // get infos
                ListViewTag tag = item.Tag as ListViewTag;
                string current = item.Text;

                // check if we have valid tag for the image
                if (tag.Info == null || tag.Info.Hash == null)
                {
                    continue;
                }

                // calculate new upper left corner
                Point upperLeftCorner = new Point(Settings.CustomTagNonPrintableMargin + (tagColumn * offsetIncrementX), Settings.CustomTagNonPrintableMargin + (tagRow * offsetIncrementY));

                // check if we need to start a new row
                if ((upperLeftCorner.X + Settings.CustomTagSize + Settings.CustomTagNonPrintableMargin) > page.Width.Millimeter)
                {
                    tagColumn = 0;
                    tagRow++;

                    // recalculate the upper left corner
                    upperLeftCorner = new Point(Settings.CustomTagNonPrintableMargin + (tagColumn * offsetIncrementX), Settings.CustomTagNonPrintableMargin + (tagRow * offsetIncrementY));
                }

                // check if we need to add a new page
                if ((upperLeftCorner.Y + Settings.CustomTagSize + Settings.CustomTagNonPrintableMargin) > page.Height.Millimeter)
                {
                    // add new page
                    page = document.AddPage();

                    // get graphics object
                    gfx = XGraphics.FromPdfPage(page, XGraphicsUnit.Millimeter);

                    // reset row and column
                    tagColumn = 0;
                    tagRow = 0;

                    // recalculate the upper left corner
                    upperLeftCorner = new Point(Settings.CustomTagNonPrintableMargin + (tagColumn * offsetIncrementX), Settings.CustomTagNonPrintableMargin + (tagRow * offsetIncrementY));
                }

                // insert image
                Image img = GetImage(tag.Info.Pic, tag.Info.Hash[0]);
                DrawImageScaled(gfx, img, new Point(upperLeftCorner.X, upperLeftCorner.Y), new Size(Settings.CustomTagSize, Settings.CustomTagSize));

                // draw rectangle
                if (Settings.CustomTagOutlineWidth > 0)
                {
                    DrawEllipse(gfx, Settings.CustomTagOutlineWidth, new Point(upperLeftCorner.X, upperLeftCorner.Y), new Size(Settings.CustomTagSize, Settings.CustomTagSize));
                }

                // increase local offset for next tag
                tagColumn++;
            }

            // save the document
            String filename = Path.Combine(outputLocation, Settings.CustomTagCoverFilename);
            document.Save(filename);

            // start a viewer.
            Process.Start(filename);
        }

        private Image CropCenterOfImage(Image imgToResize, Size destinationSize)
        {
            var originalWidth = imgToResize.Width;
            var originalHeight = imgToResize.Height;

            //how many units are there to make the original length
            var hRatio = (float)originalHeight / destinationSize.Height;
            var wRatio = (float)originalWidth / destinationSize.Width;

            //get the shorter side
            var ratio = Math.Min(hRatio, wRatio);

            var hScale = Convert.ToInt32(destinationSize.Height * ratio);
            var wScale = Convert.ToInt32(destinationSize.Width * ratio);

            //start cropping from the center
            var startX = (originalWidth - wScale) / 2;
            var startY = (originalHeight - hScale) / 2;

            //crop the image from the specified location and size
            var sourceRectangle = new Rectangle(startX, startY, wScale, hScale);

            //the future size of the image
            var bitmap = new Bitmap(destinationSize.Width, destinationSize.Height);

            //fill-in the whole bitmap
            var destinationRectangle = new Rectangle(0, 0, bitmap.Width, bitmap.Height);

            //generate the new image
            using (var g = Graphics.FromImage(bitmap))
            {
                g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                g.DrawImage(imgToResize, destinationRectangle, sourceRectangle, GraphicsUnit.Pixel);
            }

            return bitmap;

        }

        private void DrawEllipse(XGraphics gfx, double lineWidth, Point upperLeftCorner, Size size)
        {
            XPen pen = new XPen(XColors.DarkBlue, lineWidth);
            gfx.DrawEllipse(pen, upperLeftCorner.X, upperLeftCorner.Y, size.Width, size.Height);
        }

        private void DrawImageScaled(XGraphics gfx, String filename, Point upperLeftCorner, Size size)
        {
            XImage image = XImage.FromFile(filename);
            gfx.DrawImage(image, upperLeftCorner.X, upperLeftCorner.Y, size.Width, size.Height);
        }

        private void DrawImageScaled(XGraphics gfx, Image img, Point upperLeftCorner, Size size)
        {
            MemoryStream strm = new MemoryStream();
            img.Save(strm, System.Drawing.Imaging.ImageFormat.Png);
            XImage image = XImage.FromStream(strm);
            gfx.DrawImage(image, upperLeftCorner.X, upperLeftCorner.Y, size.Width, size.Height);
        }

        private void createTagBacksideToolStripMenuItem_Click(object sender, EventArgs e)
        {
            FolderBrowserDialog dlg = new FolderBrowserDialog();

            if (dlg.ShowDialog() != DialogResult.OK)
            {
                return;
            }
            string outputLocation = dlg.SelectedPath;

            // create a new PDF document
            PdfDocument document = new PdfDocument();

            // Create an empty page
            PdfPage page = document.AddPage();

            // get an XGraphics object for drawing
            XGraphics gfx = XGraphics.FromPdfPage(page, XGraphicsUnit.Millimeter);

            int tagColumn = Settings.CustomTagInitialColumn;
            int tagRow = Settings.CustomTagInitialRow;
            int offsetIncrementX = Settings.CustomTagSize + Settings.CustomTagGapX;
            int offsetIncrementY = Settings.CustomTagSize + Settings.CustomTagGapY;

            // loop over all selected objects
            foreach (ListViewItem item in lstTonies.SelectedItems)
            {
                // get infos
                ListViewTag tag = item.Tag as ListViewTag;
                string current = item.Text;

                // check if we have valid tag for the image
                if (tag.Info == null || tag.Info.Hash == null)
                {
                    continue;
                }

                // calculate new upper left corner
                Point upperLeftCorner = new Point(Settings.CustomTagNonPrintableMargin + (tagColumn * offsetIncrementX), Settings.CustomTagNonPrintableMargin + (tagRow * offsetIncrementY));

                // check if we need to start a new row
                if ((upperLeftCorner.X + Settings.CustomTagSize + Settings.CustomTagNonPrintableMargin) > page.Width.Millimeter)
                {
                    tagColumn = 0;
                    tagRow++;

                    // recalculate the upper left corner
                    upperLeftCorner = new Point(Settings.CustomTagNonPrintableMargin + (tagColumn * offsetIncrementX), Settings.CustomTagNonPrintableMargin + (tagRow * offsetIncrementY));
                }

                // check if we need to add a new page
                if ((upperLeftCorner.Y + Settings.CustomTagSize + Settings.CustomTagNonPrintableMargin) > page.Height.Millimeter)
                {
                    // add new page
                    page = document.AddPage();

                    // get graphics object
                    gfx = XGraphics.FromPdfPage(page, XGraphicsUnit.Millimeter);

                    // reset row and column
                    tagColumn = 0;
                    tagRow = 0;

                    // recalculate the upper left corner
                    upperLeftCorner = new Point(Settings.CustomTagNonPrintableMargin + (tagColumn * offsetIncrementX), Settings.CustomTagNonPrintableMargin + (tagRow * offsetIncrementY));
                }

                // draw rectangle
                if (Settings.CustomTagOutlineWidth > 0)
                {
                    DrawEllipse(gfx, Settings.CustomTagOutlineWidth, new Point(upperLeftCorner.X, upperLeftCorner.Y), new Size(Settings.CustomTagSize, Settings.CustomTagSize));
                }

                // model ID
                XTextFormatter tf = new XTextFormatter(gfx);

                XFont font = new XFont("Times New Roman", 3, XFontStyle.Regular);
                XRect rect = new XRect(upperLeftCorner.X, upperLeftCorner.Y + 33, Settings.CustomTagSize, Settings.CustomTagSize / 2);
                tf.Alignment = XParagraphAlignment.Center;
                tf.DrawString(tag.Info.Model, font, XBrushes.Black, rect, XStringFormats.TopLeft);

                // series name
                XFont font2 = new XFont("Times New Roman", 3.5, XFontStyle.Bold);
                XRect rect2 = new XRect(upperLeftCorner.X + 1, upperLeftCorner.Y + 15, Settings.CustomTagSize - 2, Settings.CustomTagSize / 2);
                tf.Alignment = XParagraphAlignment.Center;
                tf.DrawString(tag.Info.Series, font2, XBrushes.Black, rect2, XStringFormats.TopLeft);

                // increase local offset for next tag
                tagColumn++;
            }

            // save the document
            String filename = Path.Combine(outputLocation, Settings.CustomTagCoverFilename);
            document.Save(filename);

            // start a viewer.
            Process.Start(filename);
        }

        private void downloadToniesjsonOnStartupToolStripMenuItem_Click(object sender, EventArgs e)
        {
            Settings.DownloadJson = !downloadToniesjsonOnStartupToolStripMenuItem.Checked;
            SaveSettings();
        }

        private void downloadToniesjsonNowToolStripMenuItem_Click(object sender, EventArgs e)
        {
            new SafeThread(() =>
            {
                LoadJson(true);
            }, "JSON Downloader").Start();
        }
    }
}
