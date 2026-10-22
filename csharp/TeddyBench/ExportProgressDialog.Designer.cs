namespace TeddyBench
{
    partial class ExportProgressDialog
    {
        /// <summary>
        /// Required designer variable.
        /// </summary>
        private System.ComponentModel.IContainer components = null;

        /// <summary>
        /// Clean up any resources being used.
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
        /// Required method for Designer support - do not modify
        /// the contents of this method with the code editor.
        /// </summary>
        private void InitializeComponent()
        {
            this.lblCounts = new System.Windows.Forms.Label();
            this.lblCurrent = new System.Windows.Forms.Label();
            this.lblDetail = new System.Windows.Forms.Label();
            this.progressBar1 = new System.Windows.Forms.ProgressBar();
            this.btnCancel = new System.Windows.Forms.Button();
            this.SuspendLayout();
            //
            // lblCounts
            //
            this.lblCounts.AutoEllipsis = true;
            this.lblCounts.Location = new System.Drawing.Point(12, 9);
            this.lblCounts.Name = "lblCounts";
            this.lblCounts.Size = new System.Drawing.Size(396, 15);
            this.lblCounts.TabIndex = 0;
            this.lblCounts.Text = "Finished 0 of 0 selected tonies";
            //
            // lblCurrent
            //
            this.lblCurrent.AutoEllipsis = true;
            this.lblCurrent.Location = new System.Drawing.Point(12, 27);
            this.lblCurrent.Name = "lblCurrent";
            this.lblCurrent.Size = new System.Drawing.Size(396, 15);
            this.lblCurrent.TabIndex = 1;
            this.lblCurrent.Text = "Current: -";
            //
            // lblDetail
            //
            this.lblDetail.AutoEllipsis = true;
            this.lblDetail.ForeColor = System.Drawing.SystemColors.GrayText;
            this.lblDetail.Location = new System.Drawing.Point(12, 45);
            this.lblDetail.Name = "lblDetail";
            this.lblDetail.Size = new System.Drawing.Size(396, 28);
            this.lblDetail.TabIndex = 2;
            //
            // progressBar1
            //
            this.progressBar1.Location = new System.Drawing.Point(12, 76);
            this.progressBar1.Name = "progressBar1";
            this.progressBar1.Size = new System.Drawing.Size(396, 23);
            this.progressBar1.TabIndex = 3;
            //
            // btnCancel
            //
            this.btnCancel.Location = new System.Drawing.Point(333, 105);
            this.btnCancel.Name = "btnCancel";
            this.btnCancel.Size = new System.Drawing.Size(75, 23);
            this.btnCancel.TabIndex = 4;
            this.btnCancel.Text = "Cancel";
            this.btnCancel.UseVisualStyleBackColor = true;
            this.btnCancel.Click += new System.EventHandler(this.btnCancel_Click);
            //
            // ExportProgressDialog
            //
            this.AutoScaleDimensions = new System.Drawing.SizeF(6F, 13F);
            this.AutoScaleMode = System.Windows.Forms.AutoScaleMode.Font;
            this.ClientSize = new System.Drawing.Size(420, 140);
            this.Controls.Add(this.btnCancel);
            this.Controls.Add(this.progressBar1);
            this.Controls.Add(this.lblDetail);
            this.Controls.Add(this.lblCurrent);
            this.Controls.Add(this.lblCounts);
            this.ControlBox = false;
            this.FormBorderStyle = System.Windows.Forms.FormBorderStyle.FixedDialog;
            this.MaximizeBox = false;
            this.MinimizeBox = false;
            this.Name = "ExportProgressDialog";
            this.StartPosition = System.Windows.Forms.FormStartPosition.CenterParent;
            this.Text = "Exporting...";
            this.ResumeLayout(false);

        }

        #endregion

        private System.Windows.Forms.Label lblCounts;
        private System.Windows.Forms.Label lblCurrent;
        private System.Windows.Forms.Label lblDetail;
        private System.Windows.Forms.ProgressBar progressBar1;
        private System.Windows.Forms.Button btnCancel;
    }
}
