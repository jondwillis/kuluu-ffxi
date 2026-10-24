/*
 * -----------------------------------------------------------------------------
 * AltanaListener
 * Copyright (c) 2026 Voliathon
 * * This software is provided 'as-is', without any express or implied warranty.
 * In no event will the author be held liable for any damages arising from 
 * the use of this software.
 * * LEGAL NOTICE:
 * This software is a fan-made, non-commercial project and is NOT affiliated 
 * with, authorized, sponsored, or endorsed by Square Enix Holdings Co., Ltd.
 * FINAL FANTASY XI and all associated audio, music, and assets are registered 
 * trademarks of Square Enix Holdings Co., Ltd.
 * -----------------------------------------------------------------------------
 */

using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Text.Json;
using System.Windows.Forms;
using System.Windows.Media;
using Microsoft.Win32;
using System.Windows.Forms.Integration;

using Button = System.Windows.Forms.Button;
using TextBox = System.Windows.Forms.TextBox;
using Label = System.Windows.Forms.Label;
using ListViewItem = System.Windows.Forms.ListViewItem;
using CheckBox = System.Windows.Forms.CheckBox;
using Timer = System.Windows.Forms.Timer;
using Panel = System.Windows.Forms.Panel;
using Color = System.Drawing.Color;
using ListBox = System.Windows.Forms.ListBox;
using ComboBox = System.Windows.Forms.ComboBox;

namespace AltanaListener
{
    public partial class Form1 : Form
    {
        private string ffxiInstallPath = string.Empty;
        private MediaPlayer player = new MediaPlayer();
        private ListViewColumnSorter lvwColumnSorter;

        private string jsonFilePath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "track_names.json");
        private Dictionary<string, TrackInfo> savedTracks = new Dictionary<string, TrackInfo>(StringComparer.OrdinalIgnoreCase);

        private string configFilePath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "config.json");
        private AppSettings currentSettings = new AppSettings();

        private List<MusicTrack> masterTrackList = new List<MusicTrack>();

        private MusicTrack currentPlayingTrack = null;
        private Timer playbackTimer = new Timer();
        private bool isPaused = false;

        private ElementHost sliderHost;
        private System.Windows.Controls.Slider wpfSlider;
        private bool isDraggingSlider = false;

        public Form1()
        {
            InitializeComponent();

            // Automatically grab the assembly version so we never have to hardcode the label
            Version appVersion = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
            if (label1 != null) label1.Text = $"Version {appVersion.Major}.{appVersion.Minor}.{appVersion.Build}";

            LoadSettings();

            if (tbVolume != null) tbVolume.Value = currentSettings.Volume;
            if (chkLoop != null) chkLoop.Checked = currentSettings.LoopTrack;
            if (chkAutoPlay != null) chkAutoPlay.Checked = currentSettings.AutoPlay;
            if (chkFavorites != null) chkFavorites.Checked = currentSettings.FavoritesOnly;
            if (txtSearch != null) txtSearch.Text = currentSettings.LastSearch;

            this.FormClosing += (s, e) => SaveSettings();

            lvwColumnSorter = new ListViewColumnSorter();
            lstTracks.ListViewItemSorter = lvwColumnSorter;

            if (lstPlaylists != null)
            {
                lstPlaylists.SelectedIndexChanged += lstPlaylists_SelectedIndexChanged;

                ContextMenuStrip playlistMenu = new ContextMenuStrip();
                ToolStripMenuItem deletePlaylistMenuItem = new ToolStripMenuItem("Delete Playlist");
                deletePlaylistMenuItem.Click += DeletePlaylistMenuItem_Click;
                playlistMenu.Items.Add(deletePlaylistMenuItem);
                lstPlaylists.ContextMenuStrip = playlistMenu;
            }

            if (btnAbout != null)
            {
                btnAbout.Click += btnAbout_Click;
            }

            LoadSavedTracks();
            SetupListView();
            FindFFXIInstallPath();

            if (!string.IsNullOrEmpty(ffxiInstallPath))
            {
                LoadMusicFromDirectories();
            }

            ReloadPlaylistSidebar();

            player.Volume = tbVolume.Value / 100.0;
            tbVolume.Scroll += (s, e) => { player.Volume = tbVolume.Value / 100.0; };

            playbackTimer.Interval = 250;
            playbackTimer.Tick += PlaybackTimer_Tick;

            if (btnPause != null)
            {
                btnPause.Click += btnPause_Click;
                btnPause.Enabled = false;
            }
            if (btnStop != null)
            {
                btnStop.Enabled = false;
            }

            sliderHost = new ElementHost();
            sliderHost.Dock = DockStyle.Fill;
            sliderHost.BackColor = Color.Transparent;

            wpfSlider = new System.Windows.Controls.Slider();
            wpfSlider.Minimum = 0;
            wpfSlider.IsEnabled = false;

            wpfSlider.PreviewMouseDown += (s, e) => { isDraggingSlider = true; };

            wpfSlider.PreviewMouseUp += (s, e) =>
            {
                isDraggingSlider = false;
                if (player.Source != null && player.NaturalDuration.HasTimeSpan)
                {
                    player.Position = TimeSpan.FromSeconds(wpfSlider.Value);
                    if (isPaused)
                    {
                        player.Play();
                        isPaused = false;
                        if (btnPause != null) btnPause.Text = "Pause";
                        playbackTimer.Start();
                    }
                }
            };

            sliderHost.Child = wpfSlider;

            if (panSlider != null)
            {
                panSlider.Controls.Add(sliderHost);
            }

            player.MediaEnded += (s, e) =>
            {
                if (chkLoop != null && chkLoop.Checked)
                {
                    player.Position = TimeSpan.Zero;
                    player.Play();
                    return;
                }

                if (chkAutoPlay != null && chkAutoPlay.Checked)
                {
                    if (lstTracks.SelectedIndices.Count > 0)
                    {
                        int currentIndex = lstTracks.SelectedIndices[0];
                        int nextIndex = currentIndex + 1;

                        if (nextIndex < lstTracks.Items.Count)
                        {
                            lstTracks.Items[currentIndex].Selected = false;
                            lstTracks.Items[nextIndex].Selected = true;
                            lstTracks.Items[nextIndex].EnsureVisible();

                            if (lstTracks.Items[nextIndex].Tag is MusicTrack nextTrack)
                            {
                                PlayTrack(nextTrack);
                            }
                        }
                    }
                }
                else
                {
                    playbackTimer.Stop();
                    if (lblNowPlaying != null) lblNowPlaying.Text = "Status: Stopped";

                    if (btnPause != null)
                    {
                        btnPause.Enabled = false;
                        btnPause.Text = "Pause";
                    }
                    if (btnStop != null) btnStop.Enabled = false;

                    if (wpfSlider != null)
                    {
                        wpfSlider.IsEnabled = false;
                        wpfSlider.Value = 0;
                    }
                    isPaused = false;
                }
            };

            if (chkFavorites != null)
            {
                chkFavorites.CheckedChanged += (s, e) => ApplySearchFilter(txtSearch.Text);
            }
        }

        private void btnAbout_Click(object sender, EventArgs e)
        {
            string disclaimerText =
                "AltanaListener - Created by Voliathon\n\n" +
                "A free, fan-developed local audio player for Final Fantasy XI.\n\n" +
                "LEGAL NOTICE:\n" +
                "This software is NOT affiliated with, authorized, sponsored, or endorsed by Square Enix Holdings Co., Ltd. " +
                "All FFXI assets are registered trademarks of Square Enix.\n\n" +
                "AltanaListener does not distribute copyrighted audio. It only reads files from a legally obtained, local installation. " +
                "This software is provided 'as-is' without warranty of any kind.\n\n" +
                "Would you like to open the README.md file to view the full Legal, Copyright, & Liability Disclaimer?";

            DialogResult result = MessageBox.Show(disclaimerText, "About & Legal", MessageBoxButtons.YesNo, MessageBoxIcon.Information);

            if (result == DialogResult.Yes)
            {
                string readmePath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "README.md");
                if (File.Exists(readmePath))
                {
                    Process.Start(new ProcessStartInfo(readmePath) { UseShellExecute = true });
                }
                else
                {
                    MessageBox.Show("README.md could not be found in the application directory.", "File Missing", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                }
            }
        }

        private void DeletePlaylistMenuItem_Click(object sender, EventArgs e)
        {
            if (lstPlaylists.SelectedItem == null) return;
            string selected = lstPlaylists.SelectedItem.ToString();

            if (selected == "All Tracks" || selected == "Favorites")
            {
                MessageBox.Show("You cannot delete the default categories.", "Action Denied", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return;
            }

            var confirmResult = MessageBox.Show($"Are you sure you want to delete the '{selected}' playlist?", "Confirm Delete", MessageBoxButtons.YesNo, MessageBoxIcon.Question);
            if (confirmResult == DialogResult.Yes)
            {
                currentSettings.Playlists.Remove(selected);
                SaveSettings();
                ReloadPlaylistSidebar();
                lstPlaylists.SelectedIndex = 0;
            }
        }

        private void RemoveFromPlaylistMenu_Click(object sender, EventArgs e)
        {
            if (lstTracks.SelectedItems.Count == 0 || lstPlaylists.SelectedItem == null) return;
            string currentPlaylist = lstPlaylists.SelectedItem.ToString();

            if (currentPlaylist == "All Tracks" || currentPlaylist == "Favorites")
            {
                MessageBox.Show("To remove from Favorites, just unstar the track. Tracks cannot be removed from 'All Tracks'.", "Invalid Action", MessageBoxButtons.OK, MessageBoxIcon.Information);
                return;
            }

            if (lstTracks.SelectedItems[0].Tag is MusicTrack track)
            {
                if (currentSettings.Playlists.ContainsKey(currentPlaylist))
                {
                    currentSettings.Playlists[currentPlaylist].Remove(track.TrackId); // UPDATED to use TrackId
                    SaveSettings();
                    ApplySearchFilter(txtSearch.Text);
                }
            }
        }

        private void ReloadPlaylistSidebar()
        {
            if (lstPlaylists == null) return;

            lstPlaylists.BeginUpdate();
            lstPlaylists.Items.Clear();

            lstPlaylists.Items.Add("All Tracks");
            lstPlaylists.Items.Add("Favorites");

            foreach (var playlistName in currentSettings.Playlists.Keys)
            {
                lstPlaylists.Items.Add(playlistName);
            }

            lstPlaylists.EndUpdate();

            if (lstPlaylists.SelectedIndex == -1)
            {
                lstPlaylists.SelectedIndex = 0;
            }
        }

        private void lstPlaylists_SelectedIndexChanged(object sender, EventArgs e)
        {
            ApplySearchFilter(txtSearch != null ? txtSearch.Text : "");
        }

        private void LoadSettings()
        {
            if (File.Exists(configFilePath))
            {
                try
                {
                    string json = File.ReadAllText(configFilePath);
                    currentSettings = JsonSerializer.Deserialize<AppSettings>(json) ?? new AppSettings();
                }
                catch { }
            }
        }

        private void SaveSettings()
        {
            try
            {
                currentSettings.Volume = tbVolume != null ? tbVolume.Value : 50;
                currentSettings.LoopTrack = chkLoop != null && chkLoop.Checked;
                currentSettings.AutoPlay = chkAutoPlay != null && chkAutoPlay.Checked;
                currentSettings.FavoritesOnly = chkFavorites != null && chkFavorites.Checked;
                currentSettings.LastSearch = txtSearch != null ? txtSearch.Text : "";

                var options = new JsonSerializerOptions { WriteIndented = true };
                string json = JsonSerializer.Serialize(currentSettings, options);
                File.WriteAllText(configFilePath, json);
            }
            catch { }
        }

        private void PlaybackTimer_Tick(object sender, EventArgs e)
        {
            if (currentPlayingTrack != null && player.Source != null && player.NaturalDuration.HasTimeSpan && lblNowPlaying != null)
            {
                TimeSpan currentPos = player.Position;
                TimeSpan totalTime = player.NaturalDuration.TimeSpan;

                string displayName = !string.IsNullOrWhiteSpace(currentPlayingTrack.TrackName)
                    ? currentPlayingTrack.TrackName
                    : currentPlayingTrack.FileName;

                lblNowPlaying.Text = $"Now Playing: {displayName}  [{currentPos.ToString(@"mm\:ss")} / {totalTime.ToString(@"mm\:ss")}]";

                if (wpfSlider != null && !isDraggingSlider)
                {
                    int maxSeconds = (int)totalTime.TotalSeconds;
                    int currentSeconds = (int)currentPos.TotalSeconds;

                    if (maxSeconds > 0)
                    {
                        wpfSlider.Maximum = maxSeconds;
                        if (currentSeconds <= maxSeconds && currentSeconds >= 0)
                        {
                            wpfSlider.Value = currentSeconds;
                        }
                    }
                }
            }
        }

        private void LoadSavedTracks()
        {
            if (File.Exists(jsonFilePath))
            {
                try
                {
                    string json = File.ReadAllText(jsonFilePath);
                    savedTracks = JsonSerializer.Deserialize<Dictionary<string, TrackInfo>>(json)
                                  ?? new Dictionary<string, TrackInfo>(StringComparer.OrdinalIgnoreCase);
                }
                catch { }
            }
        }

        private void SaveTracksToFile()
        {
            try
            {
                var options = new JsonSerializerOptions { WriteIndented = true };
                string json = JsonSerializer.Serialize(savedTracks, options);
                File.WriteAllText(jsonFilePath, json);
            }
            catch (Exception ex)
            {
                MessageBox.Show($"Error saving track names: {ex.Message}", "Save Error", MessageBoxButtons.OK, MessageBoxIcon.Error);
            }
        }

        private void SetupListView()
        {
            lstTracks.View = View.Details;
            lstTracks.FullRowSelect = true;
            lstTracks.GridLines = true;
            lstTracks.HideSelection = false;

            lstTracks.Columns.Add("Fav", 40);
            lstTracks.Columns.Add("Sound Folder", 90);
            lstTracks.Columns.Add("File Name", 120);
            lstTracks.Columns.Add("Track Name", 200);
            lstTracks.Columns.Add("Track Author", 150);
            lstTracks.Columns.Add("Size", 80);
            lstTracks.Columns.Add("Location", 300);

            lstTracks.ColumnClick += new ColumnClickEventHandler(lstTracks_ColumnClick);
            lstTracks.MouseClick += lstTracks_MouseClick;

            ContextMenuStrip trackMenu = new ContextMenuStrip();

            ToolStripMenuItem favMenuItem = new ToolStripMenuItem("Toggle Favorite");
            favMenuItem.Click += FavMenuItem_Click;
            trackMenu.Items.Add(favMenuItem);

            ToolStripMenuItem addToPlaylistMenu = new ToolStripMenuItem("Add to Playlist...");
            addToPlaylistMenu.Click += AddToPlaylistMenu_Click;
            trackMenu.Items.Add(addToPlaylistMenu);

            ToolStripMenuItem removeFromPlaylistMenu = new ToolStripMenuItem("Remove from Current Playlist");
            removeFromPlaylistMenu.Click += RemoveFromPlaylistMenu_Click;
            trackMenu.Items.Add(removeFromPlaylistMenu);

            trackMenu.Items.Add(new ToolStripSeparator());

            ToolStripMenuItem editMenuItem = new ToolStripMenuItem("Edit Track Info...");
            editMenuItem.Click += EditMenuItem_Click;
            trackMenu.Items.Add(editMenuItem);

            ToolStripMenuItem exportMenuItem = new ToolStripMenuItem("Export Track as WAV...");
            exportMenuItem.Click += ExportMenuItem_Click;
            trackMenu.Items.Add(exportMenuItem);

            lstTracks.ContextMenuStrip = trackMenu;
        }

        private void AddToPlaylistMenu_Click(object sender, EventArgs e)
        {
            if (lstTracks.SelectedItems.Count > 0 && lstTracks.SelectedItems[0].Tag is MusicTrack track)
            {
                using (PlaylistDialog dlg = new PlaylistDialog(currentSettings.Playlists.Keys))
                {
                    if (dlg.ShowDialog() == DialogResult.OK && !string.IsNullOrWhiteSpace(dlg.PlaylistName))
                    {
                        string plName = dlg.PlaylistName;

                        if (!currentSettings.Playlists.ContainsKey(plName))
                        {
                            currentSettings.Playlists[plName] = new List<string>();
                        }

                        if (!currentSettings.Playlists[plName].Contains(track.TrackId)) // UPDATED to use TrackId
                        {
                            currentSettings.Playlists[plName].Add(track.TrackId); // UPDATED to use TrackId
                            SaveSettings();
                            ReloadPlaylistSidebar();
                            MessageBox.Show($"Added to '{plName}'!", "Playlist Updated", MessageBoxButtons.OK, MessageBoxIcon.Information);
                        }
                    }
                }
            }
        }

        private void lstTracks_MouseClick(object sender, MouseEventArgs e)
        {
            ListViewHitTestInfo info = lstTracks.HitTest(e.Location);
            if (info.Item != null && info.SubItem == info.Item.SubItems[0])
            {
                if (info.Item.Tag is MusicTrack track)
                {
                    ToggleFavorite(track, info.Item);
                }
            }
        }

        private void FavMenuItem_Click(object sender, EventArgs e)
        {
            if (lstTracks.SelectedItems.Count > 0 && lstTracks.SelectedItems[0].Tag is MusicTrack track)
            {
                ToggleFavorite(track, lstTracks.SelectedItems[0]);
            }
        }

        private void ToggleFavorite(MusicTrack track, ListViewItem item)
        {
            track.IsFavorite = !track.IsFavorite;
            item.Text = track.IsFavorite ? "★" : "☆";

            savedTracks[track.TrackId] = new TrackInfo // UPDATED to use TrackId
            {
                TrackName = track.TrackName,
                TrackAuthor = track.TrackAuthor,
                IsFavorite = track.IsFavorite
            };
            SaveTracksToFile();

            if (chkFavorites != null && chkFavorites.Checked && !track.IsFavorite)
            {
                ApplySearchFilter(txtSearch.Text);
            }
        }

        private void EditMenuItem_Click(object sender, EventArgs e)
        {
            if (lstTracks.SelectedItems.Count > 0)
            {
                ListViewItem item = lstTracks.SelectedItems[0];
                if (item.Tag is MusicTrack track)
                {
                    using (EditDialog dlg = new EditDialog(track.FileName, track.TrackName, track.TrackAuthor))
                    {
                        if (dlg.ShowDialog() == DialogResult.OK)
                        {
                            track.TrackName = dlg.NewTrackName;
                            track.TrackAuthor = dlg.NewTrackAuthor;

                            item.SubItems[3].Text = track.TrackName;
                            item.SubItems[4].Text = track.TrackAuthor;

                            savedTracks[track.TrackId] = new TrackInfo // UPDATED to use TrackId
                            {
                                TrackName = track.TrackName,
                                TrackAuthor = track.TrackAuthor,
                                IsFavorite = track.IsFavorite
                            };
                            SaveTracksToFile();
                        }
                    }
                }
            }
        }

        private void FindFFXIInstallPath()
        {
            string registryKey = @"SOFTWARE\WOW6432Node\PlayOnlineUS\InstallFolder";
            try
            {
                using (RegistryKey key = Registry.LocalMachine.OpenSubKey(registryKey))
                {
                    if (key != null)
                    {
                        string path = key.GetValue("0001") as string;
                        if (!string.IsNullOrEmpty(path) && Directory.Exists(path))
                        {
                            ffxiInstallPath = path;
                            return;
                        }
                    }
                }
            }
            catch (Exception ex) { MessageBox.Show($"Error reading registry: {ex.Message}"); }
            PromptUserForPath();
        }

        private void PromptUserForPath()
        {
            using (FolderBrowserDialog folderDialog = new FolderBrowserDialog())
            {
                folderDialog.Description = "Select your FINAL FANTASY XI installation folder";
                if (folderDialog.ShowDialog() == DialogResult.OK) { ffxiInstallPath = folderDialog.SelectedPath; }
                else { Application.Exit(); }
            }
        }

        private void LoadMusicFromDirectories()
        {
            masterTrackList.Clear();

            string[] soundFolders = { "sound", "sound2", "sound3", "sound4", "sound5", "sound6", "sound9" };

            foreach (string sFolder in soundFolders)
            {
                string musicPath = Path.Combine(ffxiInstallPath, sFolder, "win", "music", "data");

                if (Directory.Exists(musicPath))
                {
                    string[] bgwFiles = Directory.GetFiles(musicPath, "*.bgw");

                    foreach (string file in bgwFiles)
                    {
                        FileInfo info = new FileInfo(file);
                        string fileName = Path.GetFileName(file);

                        string formattedSize = info.Length >= 1048576
                            ? (info.Length / 1048576.0).ToString("0.00") + " MB"
                            : (info.Length / 1024.0).ToString("0.00") + " KB";

                        string loadedName = "";
                        string loadedAuthor = "";
                        bool loadedFav = false;

                        // UPDATED to load using TrackId
                        string trackId = $"{sFolder}_{fileName}";

                        if (savedTracks.TryGetValue(trackId, out TrackInfo savedInfo))
                        {
                            loadedName = savedInfo.TrackName;
                            loadedAuthor = savedInfo.TrackAuthor;
                            loadedFav = savedInfo.IsFavorite;
                        }

                        MusicTrack track = new MusicTrack
                        {
                            SoundFolder = sFolder,
                            FileName = fileName,
                            TrackName = loadedName,
                            TrackAuthor = loadedAuthor,
                            IsFavorite = loadedFav,
                            SizeString = formattedSize,
                            FilePath = file,
                            ExactByteSize = info.Length
                        };

                        masterTrackList.Add(track);
                    }
                }
            }

            ApplySearchFilter(currentSettings.LastSearch);
        }

        private void txtSearch_TextChanged(object sender, EventArgs e)
        {
            ApplySearchFilter(txtSearch.Text);
        }

        private void ApplySearchFilter(string searchTerm)
        {
            lstTracks.BeginUpdate();
            lstTracks.Items.Clear();

            searchTerm = searchTerm.ToLower();

            string selectedCategory = lstPlaylists != null && lstPlaylists.SelectedItem != null
                                      ? lstPlaylists.SelectedItem.ToString()
                                      : "All Tracks";

            bool favsOnly = (chkFavorites != null && chkFavorites.Checked) || selectedCategory == "Favorites";

            HashSet<string> customPlaylistFiles = null;
            if (selectedCategory != "All Tracks" && selectedCategory != "Favorites" && currentSettings.Playlists.ContainsKey(selectedCategory))
            {
                customPlaylistFiles = new HashSet<string>(currentSettings.Playlists[selectedCategory], StringComparer.OrdinalIgnoreCase);
            }

            foreach (var track in masterTrackList)
            {
                if (favsOnly && !track.IsFavorite) continue;

                // UPDATED to filter by TrackId
                if (customPlaylistFiles != null && !customPlaylistFiles.Contains(track.TrackId)) continue;

                if (string.IsNullOrWhiteSpace(searchTerm) ||
                    track.FileName.ToLower().Contains(searchTerm) ||
                    track.TrackName.ToLower().Contains(searchTerm) ||
                    track.TrackAuthor.ToLower().Contains(searchTerm) ||
                    track.SoundFolder.ToLower().Contains(searchTerm))
                {
                    ListViewItem item = new ListViewItem(track.IsFavorite ? "★" : "☆");
                    item.SubItems.Add(track.SoundFolder);
                    item.SubItems.Add(track.FileName);
                    item.SubItems.Add(track.TrackName);
                    item.SubItems.Add(track.TrackAuthor);
                    item.SubItems.Add(track.SizeString);
                    item.SubItems.Add(track.FilePath);

                    item.Tag = track;
                    lstTracks.Items.Add(item);
                }
            }

            lstTracks.EndUpdate();
        }

        private void lstTracks_ColumnClick(object sender, ColumnClickEventArgs e)
        {
            if (e.Column == lvwColumnSorter.SortColumn)
            {
                lvwColumnSorter.Order = (lvwColumnSorter.Order == SortOrder.Ascending) ? SortOrder.Descending : SortOrder.Ascending;
            }
            else
            {
                lvwColumnSorter.SortColumn = e.Column;
                lvwColumnSorter.Order = SortOrder.Ascending;
            }
            lstTracks.Sort();
        }

        private void ExportMenuItem_Click(object sender, EventArgs e)
        {
            if (lstTracks.SelectedItems.Count > 0 && lstTracks.SelectedItems[0].Tag is MusicTrack track)
            {
                ExportTrack(track);
            }
        }

        private void btnExport_Click(object sender, EventArgs e)
        {
            if (lstTracks.SelectedItems.Count > 0 && lstTracks.SelectedItems[0].Tag is MusicTrack track)
            {
                ExportTrack(track);
            }
            else
            {
                MessageBox.Show("Please select a track to export first.", "No Track Selected", MessageBoxButtons.OK, MessageBoxIcon.Warning);
            }
        }

        private string GetSafeFilename(string filename)
        {
            return string.Join("_", filename.Split(Path.GetInvalidFileNameChars()));
        }

        private string GenerateSmartFileName(MusicTrack track)
        {
            if (!string.IsNullOrWhiteSpace(track.TrackName))
            {
                string authorPart = string.IsNullOrWhiteSpace(track.TrackAuthor) ? "" : $" - {track.TrackAuthor}";
                string smartName = $"[{track.SoundFolder}] {track.TrackName}{authorPart}.wav";
                return GetSafeFilename(smartName);
            }
            else
            {
                return Path.GetFileNameWithoutExtension(track.FileName) + ".wav";
            }
        }

        private void ExportTrack(MusicTrack track)
        {
            using (System.Windows.Forms.SaveFileDialog saveDialog = new System.Windows.Forms.SaveFileDialog())
            {
                saveDialog.Filter = "WAV Audio File|*.wav";
                saveDialog.Title = "Export Track as WAV";
                saveDialog.FileName = GenerateSmartFileName(track);

                if (saveDialog.ShowDialog() == DialogResult.OK)
                {
                    string vgmstreamPath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "Dependencies", "vgmstream-cli.exe");

                    if (!File.Exists(vgmstreamPath)) return;

                    try
                    {
                        ProcessStartInfo startInfo = new ProcessStartInfo
                        {
                            FileName = vgmstreamPath,
                            Arguments = $"-o \"{saveDialog.FileName}\" \"{track.FilePath}\"",
                            CreateNoWindow = true,
                            UseShellExecute = false
                        };

                        using (Process process = Process.Start(startInfo))
                        {
                            process.WaitForExit();
                        }

                        MessageBox.Show("Track successfully exported!", "Export Complete", MessageBoxButtons.OK, MessageBoxIcon.Information);
                    }
                    catch (Exception ex)
                    {
                        MessageBox.Show($"Error exporting track: {ex.Message}", "Export Error", MessageBoxButtons.OK, MessageBoxIcon.Error);
                    }
                }
            }
        }

        private void btnPause_Click(object sender, EventArgs e)
        {
            if (currentPlayingTrack != null)
            {
                if (!isPaused)
                {
                    player.Pause();
                    isPaused = true;
                    btnPause.Text = "Resume";
                    playbackTimer.Stop();
                }
                else
                {
                    player.Play();
                    isPaused = false;
                    btnPause.Text = "Pause";
                    playbackTimer.Start();
                }
            }
        }

        private void btnPlay_Click(object sender, EventArgs e)
        {
            if (lstTracks.SelectedItems.Count > 0 && lstTracks.SelectedItems[0].Tag is MusicTrack selectedTrack)
            {
                PlayTrack(selectedTrack);
            }
        }

        private void btnStop_Click(object sender, EventArgs e)
        {
            player.Stop();
            playbackTimer.Stop();
            if (lblNowPlaying != null) lblNowPlaying.Text = "Status: Stopped";

            isPaused = false;
            if (btnPause != null)
            {
                btnPause.Enabled = false;
                btnPause.Text = "Pause";
            }
            if (btnStop != null) btnStop.Enabled = false;

            if (wpfSlider != null)
            {
                wpfSlider.IsEnabled = false;
                wpfSlider.Value = 0;
            }
        }

        private void PlayTrack(MusicTrack track)
        {
            player.Stop();
            player.Close();
            playbackTimer.Stop();

            isPaused = false;
            if (btnPause != null)
            {
                btnPause.Enabled = true;
                btnPause.Text = "Pause";
            }
            if (btnStop != null) btnStop.Enabled = true;

            if (wpfSlider != null) wpfSlider.IsEnabled = true;

            currentPlayingTrack = track;

            string vgmstreamPath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "Dependencies", "vgmstream-cli.exe");
            if (!File.Exists(vgmstreamPath)) return;

            try
            {
                string tempDir = Path.GetTempPath();
                try
                {
                    foreach (string oldFile in Directory.GetFiles(tempDir, "altana_temp_*.wav"))
                    {
                        try { File.Delete(oldFile); } catch { }
                    }
                }
                catch { }

                string uniqueTempWav = Path.Combine(tempDir, $"altana_temp_{Guid.NewGuid()}.wav");

                ProcessStartInfo startInfo = new ProcessStartInfo
                {
                    FileName = vgmstreamPath,
                    Arguments = $"-o \"{uniqueTempWav}\" \"{track.FilePath}\"",
                    CreateNoWindow = true,
                    UseShellExecute = false
                };

                using (Process process = Process.Start(startInfo)) { process.WaitForExit(); }

                if (File.Exists(uniqueTempWav))
                {
                    player.Open(new Uri(uniqueTempWav));
                    player.Volume = tbVolume.Value / 100.0;
                    player.Play();

                    playbackTimer.Start();
                }
            }
            catch (Exception ex) { MessageBox.Show($"Error playing track: {ex.Message}"); }
        }

        // Ghost events
        private void Form1_Load(object sender, EventArgs e) { }
        private void listBox1_SelectedIndexChanged(object sender, EventArgs e) { }
        private void trackBar1_Scroll(object sender, EventArgs e) { }
        private void tbVolume_Scroll(object sender, EventArgs e) { }
        private void label1_Click(object sender, EventArgs e) { }
        private void chkFavorites_CheckedChanged(object sender, EventArgs e) { }
        private void chkLoop_CheckedChanged(object sender, EventArgs e) { }
        private void chkAutoPlay_CheckedChanged(object sender, EventArgs e) { }
        private void label3_Click(object sender, EventArgs e) { }
        private void textBox1_TextChanged(object sender, EventArgs e) { }
    }

    public class MusicTrack
    {
        public string SoundFolder { get; set; }
        public string FileName { get; set; }

        // NEW: Creates a unique ID like "sound2_music181.bgw" to prevent overlaps
        public string TrackId => $"{SoundFolder}_{FileName}";

        public string TrackName { get; set; }
        public string TrackAuthor { get; set; }
        public bool IsFavorite { get; set; }
        public string SizeString { get; set; }
        public string FilePath { get; set; }
        public long ExactByteSize { get; set; }
    }

    public class TrackInfo
    {
        public string TrackName { get; set; } = "";
        public string TrackAuthor { get; set; } = "";
        public bool IsFavorite { get; set; } = false;
    }

    public class AppSettings
    {
        public int Volume { get; set; } = 50;
        public bool LoopTrack { get; set; } = false;
        public bool AutoPlay { get; set; } = false;
        public bool FavoritesOnly { get; set; } = false;
        public string LastSearch { get; set; } = "";

        public Dictionary<string, List<string>> Playlists { get; set; } = new Dictionary<string, List<string>>(StringComparer.OrdinalIgnoreCase);
    }

    public class PlaylistDialog : Form
    {
        public string PlaylistName { get; private set; }
        private ComboBox cmbPlaylists;

        public PlaylistDialog(IEnumerable<string> existingPlaylists)
        {
            this.Text = "Add to Playlist";
            this.Size = new Size(300, 160);
            this.FormBorderStyle = FormBorderStyle.FixedDialog;
            this.StartPosition = FormStartPosition.CenterParent;
            this.MaximizeBox = false;
            this.MinimizeBox = false;

            Label lbl = new Label { Text = "Select or type a new playlist name:", Left = 15, Top = 15, Width = 250 };

            cmbPlaylists = new ComboBox { Left = 15, Top = 40, Width = 250 };
            foreach (var pl in existingPlaylists) cmbPlaylists.Items.Add(pl);

            Button btnOk = new Button { Text = "OK", Left = 100, Top = 80, Width = 75, DialogResult = DialogResult.OK };
            Button btnCancel = new Button { Text = "Cancel", Left = 190, Top = 80, Width = 75, DialogResult = DialogResult.Cancel };

            btnOk.Click += (s, e) => { PlaylistName = cmbPlaylists.Text.Trim(); };

            this.Controls.Add(lbl);
            this.Controls.Add(cmbPlaylists);
            this.Controls.Add(btnOk);
            this.Controls.Add(btnCancel);

            this.AcceptButton = btnOk;
            this.CancelButton = btnCancel;
        }
    }

    public class EditDialog : Form
    {
        public string NewTrackName { get; private set; }
        public string NewTrackAuthor { get; private set; }
        private TextBox txtName;
        private TextBox txtAuthor;

        public EditDialog(string fileName, string currentName, string currentAuthor)
        {
            this.Text = $"Edit Info: {fileName}";
            this.Size = new Size(380, 180);
            this.FormBorderStyle = FormBorderStyle.FixedDialog;
            this.StartPosition = FormStartPosition.CenterParent;
            this.MaximizeBox = false;
            this.MinimizeBox = false;

            Label lblName = new Label { Text = "Track Name:", Left = 15, Top = 25, Width = 80 };
            txtName = new TextBox { Text = currentName, Left = 100, Top = 22, Width = 240 };

            Label lblAuthor = new Label { Text = "Track Author:", Left = 15, Top = 65, Width = 80 };
            txtAuthor = new TextBox { Text = currentAuthor, Left = 100, Top = 62, Width = 240 };

            Button btnOk = new Button { Text = "OK", Left = 180, Top = 100, Width = 75, DialogResult = DialogResult.OK };
            Button btnCancel = new Button { Text = "Cancel", Left = 265, Top = 100, Width = 75, DialogResult = DialogResult.Cancel };

            btnOk.Click += (s, e) => {
                NewTrackName = txtName.Text;
                NewTrackAuthor = txtAuthor.Text;
            };

            this.Controls.Add(lblName);
            this.Controls.Add(txtName);
            this.Controls.Add(lblAuthor);
            this.Controls.Add(txtAuthor);
            this.Controls.Add(btnOk);
            this.Controls.Add(btnCancel);

            this.AcceptButton = btnOk;
            this.CancelButton = btnCancel;
        }
    }

    public class ListViewColumnSorter : IComparer
    {
        private int ColumnToSort = 0;
        private SortOrder OrderOfSort = SortOrder.None;
        private CaseInsensitiveComparer ObjectCompare = new CaseInsensitiveComparer();

        public int Compare(object x, object y)
        {
            int compareResult = ObjectCompare.Compare(((ListViewItem)x).SubItems[ColumnToSort].Text, ((ListViewItem)y).SubItems[ColumnToSort].Text);
            if (OrderOfSort == SortOrder.Ascending) return compareResult;
            else if (OrderOfSort == SortOrder.Descending) return (-compareResult);
            else return 0;
        }

        public int SortColumn { set { ColumnToSort = value; } get { return ColumnToSort; } }
        public SortOrder Order { set { OrderOfSort = value; } get { return OrderOfSort; } }
    }
}