using System;
using System.IO;
using System.Reflection;
using System.Windows.Forms;
using Corel.Interop.VGCore;

namespace CorelDeepnest
{
    public partial class Main
    {
        private void Startup()
        {
        }

        [CgsAddInMacro]
        public void TestDeepnestConnection()
        {
            RunRuntime("TestDeepnestConnection");
        }

        [CgsAddInMacro]
        public void NestSelectedShapes()
        {
            RunRuntime("NestSelectedShapes");
        }

        private void RunRuntime(string command)
        {
            string runtimeDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "CorelDeepnest",
                "Runtime");
            string pointerPath = Path.Combine(runtimeDirectory, "current.txt");

            AppDomain runtimeDomain = null;
            try
            {
                if (!File.Exists(pointerPath))
                {
                    InstallBundledRuntime(runtimeDirectory, pointerPath);
                }

                string runtimeVersion = File.ReadAllText(pointerPath).Trim();
                if (Path.GetFileName(runtimeVersion) != runtimeVersion)
                {
                    throw new InvalidDataException("Invalid CorelDeepnest runtime pointer.");
                }

                string runtimePath = Path.Combine(
                    runtimeDirectory, runtimeVersion, "CorelDeepnest.Runtime.dll");
                if (!File.Exists(runtimePath))
                {
                    InstallBundledRuntime(runtimeDirectory, pointerPath);
                    runtimeVersion = File.ReadAllText(pointerPath).Trim();
                    runtimePath = Path.Combine(
                        runtimeDirectory, runtimeVersion, "CorelDeepnest.Runtime.dll");
                }

                runtimeDomain = AppDomain.CreateDomain(
                    "CorelDeepnest.Runtime." + Guid.NewGuid().ToString("N"),
                    null,
                    new AppDomainSetup { ApplicationBase = Path.GetDirectoryName(runtimePath) });

                runtimeDomain.CreateInstanceFrom(
                    runtimePath,
                    "CorelDeepnest.Runtime.RuntimeEntry",
                    false,
                    BindingFlags.Public | BindingFlags.Instance | BindingFlags.CreateInstance,
                    null,
                    new object[] { command, CreateGateway() },
                    null,
                    null);
            }
            catch (Exception error)
            {
                string errorLog = TryWriteErrorLog(error);
                MessageBox.Show(
                    "CorelDeepnest failed" + Environment.NewLine + Environment.NewLine +
                    GetRootMessage(error) +
                    (errorLog == null ? string.Empty :
                        Environment.NewLine + Environment.NewLine + "Full error: " + errorLog),
                    "CorelDeepnest",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Error);
            }
            finally
            {
                if (runtimeDomain != null)
                {
                    try
                    {
                        AppDomain.Unload(runtimeDomain);
                    }
                    catch (Exception unloadError)
                    {
                        MessageBox.Show(
                            "Runtime finished, but its AppDomain could not be unloaded." +
                            Environment.NewLine + Environment.NewLine + unloadError.Message,
                            "CorelDeepnest",
                            MessageBoxButtons.OK,
                            MessageBoxIcon.Warning);
                    }
                }
            }
        }

        private object CreateGateway()
        {
            using (Stream source = Assembly.GetExecutingAssembly()
                .GetManifestResourceStream("CorelDeepnest.Contracts.dll"))
            {
                if (source == null)
                {
                    throw new FileNotFoundException(
                        "The addon package does not contain its contracts DLL.");
                }

                var bytes = new byte[source.Length];
                int offset = 0;
                while (offset < bytes.Length)
                {
                    int read = source.Read(bytes, offset, bytes.Length - offset);
                    if (read == 0)
                    {
                        throw new EndOfStreamException(
                            "Could not read the embedded contracts DLL.");
                    }
                    offset += read;
                }

                Assembly contracts = Assembly.Load(bytes);
                Type gatewayType = contracts.GetType(
                    "CorelDeepnest.Contracts.CorelGateway", true);
                return Activator.CreateInstance(gatewayType, new object[] { app });
            }
        }

        private static string TryWriteErrorLog(Exception error)
        {
            try
            {
                string directory = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                    "CorelDeepnest");
                Directory.CreateDirectory(directory);
                string path = Path.Combine(directory, "last-error.txt");
                File.WriteAllText(
                    path,
                    DateTime.Now.ToString("O") + Environment.NewLine + error);
                return path;
            }
            catch
            {
                return null;
            }
        }

        private static void InstallBundledRuntime(string runtimeDirectory, string pointerPath)
        {
            const string resourceName = "CorelDeepnest.Runtime.dll";
            const string runtimeVersion = "bundled";
            Directory.CreateDirectory(runtimeDirectory);
            string versionDirectory = Path.Combine(runtimeDirectory, runtimeVersion);
            Directory.CreateDirectory(versionDirectory);

            using (Stream source = Assembly.GetExecutingAssembly()
                .GetManifestResourceStream(resourceName))
            {
                if (source == null)
                {
                    throw new FileNotFoundException(
                        "The addon package does not contain its bundled Runtime DLL.");
                }

                string runtimePath = Path.Combine(
                    versionDirectory, "CorelDeepnest.Runtime.dll");
                string runtimeTemporaryPath = runtimePath + ".tmp";
                using (var destination = new FileStream(
                    runtimeTemporaryPath, FileMode.Create, FileAccess.Write, FileShare.None))
                {
                    source.CopyTo(destination);
                }

                if (File.Exists(runtimePath))
                {
                    File.Delete(runtimePath);
                }
                File.Move(runtimeTemporaryPath, runtimePath);
            }

            using (Stream source = Assembly.GetExecutingAssembly()
                .GetManifestResourceStream("CorelDeepnest.Contracts.dll"))
            {
                if (source == null)
                {
                    throw new FileNotFoundException(
                        "The addon package does not contain its contracts DLL.");
                }

                string contractsPath = Path.Combine(
                    versionDirectory, "CorelDeepnest.Contracts.dll");
                using (var destination = new FileStream(
                    contractsPath, FileMode.Create, FileAccess.Write, FileShare.None))
                {
                    source.CopyTo(destination);
                }
            }

            string pointerTemporaryPath = pointerPath + ".tmp";
            File.WriteAllText(pointerTemporaryPath, runtimeVersion);
            if (File.Exists(pointerPath))
            {
                File.Delete(pointerPath);
            }
            File.Move(pointerTemporaryPath, pointerPath);
        }

        private static string GetRootMessage(Exception error)
        {
            while (error.InnerException != null)
            {
                error = error.InnerException;
            }

            return error.Message;
        }
    }

}
