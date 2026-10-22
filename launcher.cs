// Compiled by install.ps1 into the per-user ggpdf.exe Open with launcher.
using System;
using System.Diagnostics;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32;

[assembly: AssemblyTitle("ggpdf")]
[assembly: AssemblyDescription("ggpdf PDF viewer")]

internal static class Launcher
{
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int MessageBoxW(IntPtr window, string text, string title, uint type);

    private static string Quote(string value)
    {
        var result = new StringBuilder("\"");
        int slashes = 0;
        foreach (char character in value)
        {
            if (character == '\\')
            {
                slashes++;
                continue;
            }
            if (character == '"')
            {
                result.Append('\\', slashes * 2 + 1).Append('"');
                slashes = 0;
                continue;
            }
            result.Append('\\', slashes).Append(character);
            slashes = 0;
        }
        return result.Append('\\', slashes * 2).Append('"').ToString();
    }

    private static void Main(string[] files)
    {
        try
        {
            using (var settings = Registry.CurrentUser.OpenSubKey(@"Software\ggpdf"))
            {
                string python = settings == null ? null : settings.GetValue("Python") as string;
                string script = settings == null ? null : settings.GetValue("Script") as string;
                if (String.IsNullOrEmpty(python) || String.IsNullOrEmpty(script))
                    throw new InvalidOperationException("Re-run install.ps1 to repair ggpdf.");

                var command = new StringBuilder(Quote(script));
                foreach (string file in files)
                    command.Append(' ').Append(Quote(file));
                Process.Start(new ProcessStartInfo(python, command.ToString()) {
                    UseShellExecute = false,
                    CreateNoWindow = true
                });
            }
        }
        catch (Exception error)
        {
            MessageBoxW(IntPtr.Zero, error.Message, "ggpdf", 0x10);
        }
    }
}
