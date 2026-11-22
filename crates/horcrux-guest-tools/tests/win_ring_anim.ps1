Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @"
using System; using System.Drawing; using System.Windows.Forms; using System.Diagnostics;
public static class HxAnim { public static string Rate;
  public static void Run(int secs) {
    var f = new Form(); f.FormBorderStyle = FormBorderStyle.None; f.WindowState = FormWindowState.Maximized; f.TopMost = true; f.Show();
    var g = f.CreateGraphics(); var font = new Font("Consolas", 48); var sw = Stopwatch.StartNew(); int i = 0;
    var brush = new SolidBrush(Color.Black);
    while (sw.Elapsed.TotalSeconds < secs && f.Visible) {
      brush.Color = Color.FromArgb(40 + (i*7)%200, 40 + (i*13)%200, 40 + (i*29)%200);
      g.FillRectangle(brush, 0, 0, f.ClientSize.Width, f.ClientSize.Height);
      g.DrawString("HORCRUX RING FRAME " + i, font, Brushes.White, 40, 40);
      i++; Application.DoEvents(); System.Threading.Thread.Sleep(2);
    }
    f.Close(); Rate = i + " frames in " + sw.Elapsed.TotalSeconds.ToString("F1") + " s";
  }
}
"@
[HxAnim]::Run(40)
Invoke-RestMethod -Method Put -Uri http://10.0.2.2:8001/win-ring-anim.txt -Body ([HxAnim]::Rate) | Out-Null
