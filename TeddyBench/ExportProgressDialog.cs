using System;
using System.Windows.Forms;

namespace TeddyBench
{
    public partial class ExportProgressDialog : Form
    {
        public bool CancelRequested { get; private set; }

        private int _total;
        private int _finished;

        public ExportProgressDialog()
        {
            InitializeComponent();
        }

        /// <summary>
        /// Sets how many tonies were selected for export in total. Call once, before the loop starts.
        /// </summary>
        public void SetTotal(int total)
        {
            _total = total;
            _finished = 0;
            UpdateCounts();
        }

        /// <summary>
        /// Sets how many of the selected tonies have finished processing (exported, converted or skipped).
        /// </summary>
        public void SetFinished(int finished)
        {
            _finished = finished;
            UpdateCounts();
        }

        /// <summary>
        /// Sets the name of the tonie that is currently being processed.
        /// </summary>
        public void SetCurrentItem(string name)
        {
            lblCurrent.Text = "Current: " + name;
        }

        /// <summary>
        /// Sets an additional detail line below the current item (e.g. which track is being converted).
        /// </summary>
        public void SetDetail(string text)
        {
            lblDetail.Text = text;
        }

        private void UpdateCounts()
        {
            lblCounts.Text = "Finished " + _finished + " of " + _total + " selected tonies";

            progressBar1.Style = ProgressBarStyle.Blocks;
            progressBar1.Maximum = Math.Max(_total, 1);
            progressBar1.Value = Math.Max(0, Math.Min(_finished, progressBar1.Maximum));
        }

        private void btnCancel_Click(object sender, EventArgs e)
        {
            CancelRequested = true;
            btnCancel.Enabled = false;
            btnCancel.Text = "Cancelling...";
        }
    }
}
