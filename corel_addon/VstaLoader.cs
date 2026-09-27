using System;
using System.IO;
using System.IO.Compression;
using System.Reflection;
using System.Security.Cryptography;
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

                InstallBundledServer();

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
                    new object[] { command, CreateGateway(Path.GetDirectoryName(runtimePath)) },
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

        private object CreateGateway(string runtimeDirectory)
        {
            string contractsPath = Path.Combine(
                runtimeDirectory, "CorelDeepnest.Contracts.dll");
            if (!File.Exists(contractsPath))
            {
                throw new FileNotFoundException(
                    "The current CorelDeepnest runtime has no contracts DLL.",
                    contractsPath);
            }

            // Load from bytes so every version can coexist in Corel's main
            // AppDomain. This keeps COM access in that domain while allowing
            // the gateway implementation to change with a hot-reloaded Runtime.
            Assembly contracts = Assembly.Load(File.ReadAllBytes(contractsPath));
            Type gatewayType = contracts.GetType(
                "CorelDeepnest.Contracts.CorelGateway", true);
            return Activator.CreateInstance(gatewayType, new object[] { app });
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

        /// <summary>
        /// Extracts the bundled server (server.zip, optional) to a per-version
        /// directory under %LOCALAPPDATA%\CorelDeepnest\Server and points
        /// current.txt at it. The addon then starts node.exe from there, so no
        /// folder has to be chosen. No-op when the package carries no server.zip.
        /// </summary>
        private static void InstallBundledServer()
        {
            using (Stream source = Assembly.GetExecutingAssembly()
                .GetManifestResourceStream("server.zip"))
            {
                if (source == null)
                {
                    return;
                }

                byte[] bytes;
                using (var buffer = new MemoryStream())
                {
                    source.CopyTo(buffer);
                    bytes = buffer.ToArray();
                }

                string hash;
                using (SHA256 sha = SHA256.Create())
                {
                    hash = BitConverter.ToString(sha.ComputeHash(bytes))
                        .Replace("-", string.Empty).Substring(0, 16).ToLowerInvariant();
                }

                string root = Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                    "CorelDeepnest",
                    "Server");
                string directory = Path.Combine(root, hash);
                string pointer = Path.Combine(root, "current.txt");
                if (File.Exists(pointer) &&
                    File.ReadAllText(pointer).Trim() == hash &&
                    File.Exists(Path.Combine(directory, "node.exe")))
                {
                    return;
                }

                if (Directory.Exists(directory))
                {
                    Directory.Delete(directory, true);
                }
                Directory.CreateDirectory(directory);
                using (var buffer = new MemoryStream(bytes))
                using (var archive = new ZipArchive(buffer, ZipArchiveMode.Read))
                {
                    foreach (ZipArchiveEntry entry in archive.Entries)
                    {
                        if (string.IsNullOrEmpty(entry.Name))
                        {
                            continue; // directory entry
                        }
                        string target = Path.Combine(
                            directory,
                            entry.FullName.Replace('\\', '/')
                                .Replace('/', Path.DirectorySeparatorChar));
                        Directory.CreateDirectory(Path.GetDirectoryName(target));
                        entry.ExtractToFile(target, true);
                    }
                }

                Directory.CreateDirectory(root);
                string temporary = pointer + ".tmp";
                File.WriteAllText(temporary, hash);
                if (File.Exists(pointer))
                {
                    File.Delete(pointer);
                }
                File.Move(temporary, pointer);
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
