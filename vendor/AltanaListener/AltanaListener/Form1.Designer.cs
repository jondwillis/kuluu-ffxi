namespace AltanaListener
{
    partial class Form1
    {
        /// <summary>
        ///  Required designer variable.
        /// </summary>
        private System.ComponentModel.IContainer components = null;

        /// <summary>
        ///  Clean up any resources being used.
        /// </summary>
        /// <param name="disposing">true if managed resources should be disposed; otherwise, false.</param>
        protected override void Dispose(bool disposing)
        {
            if (disposing && (components != null))
            {
                components.Dispose();
            }
            base.Dispose(disposing);
        }

        #region Windows Form Designer generated code

        /// <summary>
        ///  Required method for Designer support - do not modify
        ///  the contents of this method with the code editor.
        /// </summary>
        private void InitializeComponent()
        {
            System.ComponentModel.ComponentResourceManager resources = new System.ComponentModel.ComponentResourceManager(typeof(Form1));
            btnPlay = new Button();
            btnStop = new Button();
            tbVolume = new TrackBar();
            lstTracks = new ListView();
            button1 = new Button();
            txtSearch = new TextBox();
            label2 = new Label();
            chkLoop = new CheckBox();
            chkFavorites = new CheckBox();
            chkAutoPlay = new CheckBox();
            lblNowPlaying = new Label();
            btnPause = new Button();
            panSlider = new Panel();
            lstPlaylists = new ListBox();
            label3 = new Label();
            textBox1 = new TextBox();
            label1 = new Label();
            btnAbout = new Button();
            ((System.ComponentModel.ISupportInitialize)tbVolume).BeginInit();
            SuspendLayout();
            // 
            // btnPlay
            // 
            btnPlay.FlatStyle = FlatStyle.Flat;
            btnPlay.ForeColor = Color.Black;
            btnPlay.Location = new Point(42, 429);
            btnPlay.Name = "btnPlay";
            btnPlay.Size = new Size(106, 40);
            btnPlay.TabIndex = 1;
            btnPlay.Text = "Play";
            btnPlay.UseVisualStyleBackColor = true;
            btnPlay.Click += btnPlay_Click;
            // 
            // btnStop
            // 
            btnStop.FlatStyle = FlatStyle.Flat;
            btnStop.ForeColor = Color.Black;
            btnStop.Location = new Point(173, 429);
            btnStop.Name = "btnStop";
            btnStop.Size = new Size(121, 40);
            btnStop.TabIndex = 2;
            btnStop.Text = "Stop";
            btnStop.UseVisualStyleBackColor = true;
            btnStop.Click += btnStop_Click;
            // 
            // tbVolume
            // 
            tbVolume.Location = new Point(417, 439);
            tbVolume.Maximum = 100;
            tbVolume.Name = "tbVolume";
            tbVolume.Size = new Size(230, 45);
            tbVolume.TabIndex = 3;
            tbVolume.TickFrequency = 10;
            tbVolume.Value = 50;
            tbVolume.Scroll += trackBar1_Scroll;
            // 
            // lstTracks
            // 
            lstTracks.Location = new Point(42, 74);
            lstTracks.Name = "lstTracks";
            lstTracks.Size = new Size(1318, 344);
            lstTracks.TabIndex = 4;
            lstTracks.UseCompatibleStateImageBehavior = false;
            // 
            // button1
            // 
            button1.FlatStyle = FlatStyle.Flat;
            button1.ForeColor = Color.Black;
            button1.Location = new Point(691, 424);
            button1.Name = "button1";
            button1.Size = new Size(75, 45);
            button1.TabIndex = 6;
            button1.Text = "Export";
            button1.UseVisualStyleBackColor = true;
            button1.Click += btnExport_Click;
            // 
            // txtSearch
            // 
            txtSearch.Location = new Point(1033, 38);
            txtSearch.Name = "txtSearch";
            txtSearch.Size = new Size(208, 23);
            txtSearch.TabIndex = 7;
            txtSearch.TextChanged += txtSearch_TextChanged;
            // 
            // label2
            // 
            label2.AutoSize = true;
            label2.ForeColor = Color.Black;
            label2.Location = new Point(982, 41);
            label2.Name = "label2";
            label2.Size = new Size(45, 15);
            label2.TabIndex = 8;
            label2.Text = "Search:";
            // 
            // chkLoop
            // 
            chkLoop.AutoSize = true;
            chkLoop.ForeColor = Color.Black;
            chkLoop.Location = new Point(305, 450);
            chkLoop.Name = "chkLoop";
            chkLoop.Size = new Size(84, 19);
            chkLoop.TabIndex = 9;
            chkLoop.Text = "Loop Track";
            chkLoop.UseVisualStyleBackColor = true;
            chkLoop.CheckedChanged += chkLoop_CheckedChanged;
            // 
            // chkFavorites
            // 
            chkFavorites.AutoSize = true;
            chkFavorites.ForeColor = Color.Black;
            chkFavorites.Location = new Point(1259, 40);
            chkFavorites.Name = "chkFavorites";
            chkFavorites.Size = new Size(101, 19);
            chkFavorites.TabIndex = 10;
            chkFavorites.Text = "Favorites Only";
            chkFavorites.UseVisualStyleBackColor = true;
            // 
            // chkAutoPlay
            // 
            chkAutoPlay.AutoSize = true;
            chkAutoPlay.ForeColor = Color.Black;
            chkAutoPlay.Location = new Point(305, 429);
            chkAutoPlay.Name = "chkAutoPlay";
            chkAutoPlay.Size = new Size(106, 19);
            chkAutoPlay.TabIndex = 11;
            chkAutoPlay.Text = "Auto-Play Next";
            chkAutoPlay.UseVisualStyleBackColor = true;
            // 
            // lblNowPlaying
            // 
            lblNowPlaying.AutoSize = true;
            lblNowPlaying.ForeColor = Color.Black;
            lblNowPlaying.Location = new Point(123, 38);
            lblNowPlaying.Name = "lblNowPlaying";
            lblNowPlaying.Size = new Size(90, 15);
            lblNowPlaying.TabIndex = 12;
            lblNowPlaying.Text = "Now Playing: --";
            lblNowPlaying.Click += label3_Click;
            // 
            // btnPause
            // 
            btnPause.FlatStyle = FlatStyle.Flat;
            btnPause.ForeColor = Color.Black;
            btnPause.Location = new Point(42, 34);
            btnPause.Name = "btnPause";
            btnPause.Size = new Size(75, 23);
            btnPause.TabIndex = 13;
            btnPause.Text = "Pause";
            btnPause.UseVisualStyleBackColor = true;
            // 
            // panSlider
            // 
            panSlider.Location = new Point(513, 38);
            panSlider.Name = "panSlider";
            panSlider.Size = new Size(463, 30);
            panSlider.TabIndex = 14;
            // 
            // lstPlaylists
            // 
            lstPlaylists.FormattingEnabled = true;
            lstPlaylists.ItemHeight = 15;
            lstPlaylists.Location = new Point(42, 511);
            lstPlaylists.Name = "lstPlaylists";
            lstPlaylists.Size = new Size(724, 304);
            lstPlaylists.TabIndex = 15;
            // 
            // label3
            // 
            label3.AutoSize = true;
            label3.Font = new Font("Segoe Script", 11.25F, FontStyle.Bold, GraphicsUnit.Point, 0);
            label3.ForeColor = Color.Black;
            label3.Location = new Point(12, 483);
            label3.Name = "label3";
            label3.Size = new Size(80, 25);
            label3.TabIndex = 16;
            label3.Text = "Playlists";
            // 
            // textBox1
            // 
            textBox1.ForeColor = SystemColors.ControlDarkDark;
            textBox1.Location = new Point(485, 486);
            textBox1.Name = "textBox1";
            textBox1.ReadOnly = true;
            textBox1.Size = new Size(281, 23);
            textBox1.TabIndex = 17;
            textBox1.Text = "Tip: Right-click a track to add it to a custom playlist.";
            textBox1.TextChanged += textBox1_TextChanged;
            // 
            // label1
            // 
            label1.AutoSize = true;
            label1.Font = new Font("Segoe Script", 9F, FontStyle.Regular, GraphicsUnit.Point, 0);
            label1.ForeColor = Color.Red;
            label1.Location = new Point(12, 9);
            label1.Name = "label1";
            label1.Size = new Size(94, 19);
            label1.TabIndex = 5;
            label1.Text = "Version 1.0.1";
            label1.Click += label1_Click;
            // 
            // btnAbout
            // 
            btnAbout.ForeColor = Color.Black;
            btnAbout.Location = new Point(1249, 424);
            btnAbout.Name = "btnAbout";
            btnAbout.Size = new Size(111, 45);
            btnAbout.TabIndex = 18;
            btnAbout.Text = "About / Legal";
            btnAbout.UseVisualStyleBackColor = true;
            // 
            // Form1
            // 
            AutoScaleDimensions = new SizeF(7F, 15F);
            AutoScaleMode = AutoScaleMode.Font;
            BackColor = Color.WhiteSmoke;
            ClientSize = new Size(1452, 854);
            Controls.Add(btnAbout);
            Controls.Add(textBox1);
            Controls.Add(label3);
            Controls.Add(lstPlaylists);
            Controls.Add(panSlider);
            Controls.Add(btnPause);
            Controls.Add(lblNowPlaying);
            Controls.Add(chkAutoPlay);
            Controls.Add(chkFavorites);
            Controls.Add(chkLoop);
            Controls.Add(label2);
            Controls.Add(txtSearch);
            Controls.Add(button1);
            Controls.Add(label1);
            Controls.Add(lstTracks);
            Controls.Add(tbVolume);
            Controls.Add(btnStop);
            Controls.Add(btnPlay);
            ForeColor = Color.White;
            Icon = (Icon)resources.GetObject("$this.Icon");
            Name = "Form1";
            Text = "AltanaListener";
            Load += Form1_Load;
            ((System.ComponentModel.ISupportInitialize)tbVolume).EndInit();
            ResumeLayout(false);
            PerformLayout();
        }

        #endregion
        private Button btnPlay;
        private Button btnStop;
        private TrackBar tbVolume;
        private ListView lstTracks;
        private Button button1;
        private TextBox txtSearch;
        private Label label2;
        private CheckBox chkLoop;
        private CheckBox chkFavorites;
        private CheckBox chkAutoPlay;
        private Label lblNowPlaying;
        private Button btnPause;
        private Panel panSlider;
        private ListBox lstPlaylists;
        private Label label3;
        private TextBox textBox1;
        private Label label1;
        private Button btnAbout;
    }
}
