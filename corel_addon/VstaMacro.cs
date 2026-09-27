using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Globalization;
using System.IO;
using System.Net.Http;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Forms;
using CorelDeepnest.Contracts;
using DrawingColor = System.Drawing.Color;

namespace CorelDeepnest.Runtime
{
    public sealed class RuntimeEntry
    {
        private static readonly HttpClient Http = new HttpClient
        {
            BaseAddress = new Uri("http://127.0.0.1:8080/"),
            Timeout = TimeSpan.FromSeconds(10)
        };

        private static readonly JavaScriptSerializer Json = CreateJsonSerializer();

        private static JavaScriptSerializer CreateJsonSerializer()
        {
            var serializer = new JavaScriptSerializer();
            serializer.MaxJsonLength = 12 * 1024 * 1024;
            return serializer;
        }

        public RuntimeEntry(string command, object gatewayObject)
        {
            Execute(command, (ICorelGateway)gatewayObject);
        }

        private sealed class GeometryPoint
        {
            public double X;
            public double Y;
        }

        private sealed class PreviewPart
        {
            public string Id;
            public List<PreviewContour> Contours;
        }

        private sealed class PreviewContour
        {
            public List<GeometryPoint> Points;
            public List<List<GeometryPoint>> Holes;
        }

        private sealed class PreviewPlacement
        {
            public string PartId;
            public int SheetInstanceId;
            public double X;
            public double Y;
            public double Rotation;
        }

        private sealed class PreviewModel
        {
            public double SheetWidth;
            public double SheetHeight;
            public double SheetGap;
            public int SheetCount;
            public List<PreviewPart> Parts;
            public List<PreviewPlacement> Placements;
        }

        private sealed class JobRunResult
        {
            public PreviewModel Preview;
            public string JobStatus;
            public string UpdatedAt;
            public string StartedAt;
            public double Fitness;
            public int Index;
            public bool Better;
        }

        private sealed class NestingOptions
        {
            public double SheetWidth;
            public double SheetHeight;
            public double Spacing;
            public int Rotations;
            public int PopulationSize;
            public int MutationRate;
            public string PlacementType;
            public bool MergeLines;
            public double CurveTolerance;
            public double TimeRatio;
            public int TimeLimitSeconds;
        }

        public void Execute(string command, ICorelGateway gateway)
        {
            if (command == "TestDeepnestConnection")
            {
                TestDeepnestConnection();
                return;
            }

            if (command == "NestSelectedShapes")
            {
                NestSelectedShapes(gateway);
                return;
            }

            throw new InvalidOperationException("Неизвестная команда CorelDeepnest: " + command);
        }

        private static void TestDeepnestConnection()
        {
            try
            {
                MessageBox.Show(
                    "Подключение к Deepnest Server — OK" + Environment.NewLine + Environment.NewLine +
                    Send("GET", "health", null),
                    "CorelDeepnest",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Information);
            }
            catch (Exception error)
            {
                ShowError("Не удалось подключиться к Deepnest Server", error);
            }
        }

        private static void NestSelectedShapes(ICorelGateway gateway)
        {
            try
            {
                using (NestingForm form = new NestingForm(gateway))
                {
                    form.ShowDialog();
                }
            }
            catch (Exception error)
            {
                ShowError("Ошибка окна Deepnest", error);
            }
        }

        private static JobRunResult RunJob(string requestJson, NestingOptions options,
            List<PreviewPart> previewParts, Action<string> jobCreated,
            Action<JobRunResult> resultUpdated, Func<bool> stopRequested)
        {
            string jobId = null;
            JobRunResult latestResult = null;
            string latestUpdate = null;

            try
            {
                string created = Send("POST", "api/v1/jobs", requestJson);
                Dictionary<string, object> createdObject =
                    Json.Deserialize<Dictionary<string, object>>(created);

                object jobIdValue;
                if (!createdObject.TryGetValue("jobId", out jobIdValue) || jobIdValue == null)
                {
                    throw new InvalidOperationException(
                        "The server accepted the job but did not return jobId." +
                        Environment.NewLine + created);
                }

                jobId = Convert.ToString(jobIdValue);
                jobCreated(jobId);

                while (true)
                {
                    if (stopRequested())
                    {
                        // Request the stop without blocking the UI: the server
                        // acknowledges immediately and tears the engine down in
                        // the background. The finally block also sends a stop.
                        RequestStop(jobId);
                        if (latestResult == null)
                        {
                            throw new OperationCanceledException(
                                "Задача остановлена до получения результата.");
                        }

                        JobRunResult stoppedResult = ReadResult(
                            jobId, options.SheetWidth, options.SheetHeight, previewParts);
                        if (latestResult == null || stoppedResult.Better)
                        {
                            latestResult = stoppedResult;
                        }
                        else
                        {
                            latestResult.JobStatus = stoppedResult.JobStatus;
                        }
                        resultUpdated(latestResult);
                        return latestResult;
                    }

                    using (HttpResponseMessage response = Http.GetAsync(
                        "api/v1/jobs/" + jobId + "/result").GetAwaiter().GetResult())
                    {
                        string body = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
                        if (response.IsSuccessStatusCode)
                        {
                            JobRunResult current = ParseJobResult(
                                body, options.SheetWidth, options.SheetHeight, previewParts);
                            if (latestUpdate != current.UpdatedAt)
                            {
                                latestUpdate = current.UpdatedAt;
                                if (latestResult == null || current.Better)
                                {
                                    latestResult = current;
                                    resultUpdated(current);
                                }
                            }

                            if (current.JobStatus == "completed" ||
                                current.JobStatus == "stopped")
                            {
                                if (latestResult == null || current.Better)
                                {
                                    latestResult = current;
                                }
                                else
                                {
                                    latestResult.JobStatus = current.JobStatus;
                                }
                                return latestResult;
                            }

                            if (current.JobStatus == "failed")
                            {
                                throw new InvalidOperationException(
                                    "The Deepnest job failed on the server.");
                            }
                        }

                        else if ((int)response.StatusCode != 409)
                        {
                            ThrowHttpError(response, body);
                        }
                    }

                    Thread.Sleep(200);
                }
            }
            finally
            {
                if (!string.IsNullOrEmpty(jobId))
                {
                    // Idempotent: stop the job if it is still running. Also a
                    // safety net for the best-effort async stop above. The
                    // server responds immediately, so this does not block.
                    try
                    {
                        Send("POST", "api/v1/jobs/" + jobId + "/stop", null);
                    }
                    catch
                    {
                    }
                }
            }
        }

        private static JobRunResult ReadResult(string jobId, double sheetWidth,
            double sheetHeight, List<PreviewPart> previewParts)
        {
            string body = Send("GET", "api/v1/jobs/" + jobId + "/result", null);
            return ParseJobResult(body, sheetWidth, sheetHeight, previewParts);
        }

        private static Dictionary<string, object> BuildRequest(ICorelGateway gateway,
            NestingOptions options, out List<PreviewPart> previewParts)
        {
            var selection = Json.Deserialize<Dictionary<string, object>>(
                gateway.CaptureSelectionSvgJson());
            var parts = new List<object>();
            previewParts = new List<PreviewPart>();
            foreach (object partValue in
                (System.Collections.IEnumerable)selection["parts"])
            {
                var capturedPart = (Dictionary<string, object>)partValue;
                parts.Add(new Dictionary<string, object>
                {
                    { "id", Convert.ToString(capturedPart["id"]) },
                    { "data", Convert.ToString(capturedPart["data"]) },
                    { "quantity", 1 },
                    { "rigid", true }
                });
            }

            string width = options.SheetWidth.ToString("R", CultureInfo.InvariantCulture);
            string height = options.SheetHeight.ToString("R", CultureInfo.InvariantCulture);
            string binSvg =
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"" + width +
                "mm\" height=\"" + height + "mm\" viewBox=\"0 0 " + width +
                " " + height + "\"><rect x=\"0\" y=\"0\" width=\"" + width +
                "\" height=\"" + height + "\"/></svg>";

            var config = BuildConfig(options);
            config.Add("units", "mm");
            config.Add("scale", 25.4);
            var request = new Dictionary<string, object>
            {
                { "input", new Dictionary<string, object>
                    {
                        { "format", "svg" },
                        { "bin", new Dictionary<string, object>
                            {
                                { "id", "sheet-1" },
                                { "data", binSvg },
                                { "mode", "auto" }
                            }
                        },
                        { "parts", parts.ToArray() }
                    }
                },
                { "config", config }
            };
            AddExecution(request, options);
            return request;
        }

        private static Dictionary<string, object> BuildConfig(NestingOptions options)
        {
            return new Dictionary<string, object>
            {
                { "spacing", options.Spacing },
                { "rotations", options.Rotations },
                { "populationSize", options.PopulationSize },
                { "mutationRate", options.MutationRate },
                { "placementType", options.PlacementType },
                { "mergeLines", options.MergeLines },
                { "curveTolerance", options.CurveTolerance },
                { "timeRatio", options.TimeRatio }
            };
        }

        private static void AddExecution(
            Dictionary<string, object> request, NestingOptions options)
        {
            if (options.TimeLimitSeconds > 0)
            {
                request.Add("execution", new Dictionary<string, object>
                {
                    { "timeLimitMs", options.TimeLimitSeconds * 1000 }
                });
            }
        }

        private static PreviewContour ParsePreviewContour(
            Dictionary<string, object> polygonTree)
        {
            var holes = new List<List<GeometryPoint>>();
            CollectChildContours(polygonTree, holes);
            return new PreviewContour
            {
                Points = ParseGeometryPoints(polygonTree["points"]),
                Holes = holes
            };
        }

        private static PreviewPart ParsePreviewPart(
            Dictionary<string, object> part)
        {
            var contours = new List<PreviewContour>();
            object treesValue;
            if (part.TryGetValue("polygontrees", out treesValue) &&
                treesValue != null)
            {
                foreach (object treeValue in
                    (System.Collections.IEnumerable)treesValue)
                {
                    contours.Add(ParsePreviewContour(
                        (Dictionary<string, object>)treeValue));
                }
            }
            else
            {
                contours.Add(ParsePreviewContour(
                    (Dictionary<string, object>)part["polygontree"]));
            }
            return new PreviewPart
            {
                Id = Convert.ToString(part["id"]),
                Contours = contours
            };
        }

        private static void CollectChildContours(
            Dictionary<string, object> polygonTree,
            List<List<GeometryPoint>> contours)
        {
            object childrenValue;
            if (!polygonTree.TryGetValue("children", out childrenValue) ||
                childrenValue == null)
            {
                return;
            }

            foreach (object childValue in
                (System.Collections.IEnumerable)childrenValue)
            {
                var child = (Dictionary<string, object>)childValue;
                contours.Add(ParseGeometryPoints(child["points"]));
                CollectChildContours(child, contours);
            }
        }

        private static List<GeometryPoint> ParseGeometryPoints(object pointsValue)
        {
            var points = new List<GeometryPoint>();
            foreach (object pointValue in
                (System.Collections.IEnumerable)pointsValue)
            {
                var point = (Dictionary<string, object>)pointValue;
                points.Add(new GeometryPoint
                {
                    X = Convert.ToDouble(point["x"]),
                    Y = Convert.ToDouble(point["y"])
                });
            }
            return points;
        }

        private static PreviewModel ParsePreview(string json, double sheetWidth,
            double sheetHeight, List<PreviewPart> parts)
        {
            const double sheetGap = 20.0;
            var root = Json.Deserialize<Dictionary<string, object>>(json);
            var placements = new List<PreviewPlacement>();
            int sheetCount = 1;
            object value;

            if (parts.Count == 0 && root.TryGetValue("parts", out value) &&
                value != null)
            {
                foreach (object item in (System.Collections.IEnumerable)value)
                {
                    var part = (Dictionary<string, object>)item;
                    parts.Add(ParsePreviewPart(part));
                }
            }

            if (root.TryGetValue("placements", out value))
            {
                foreach (object item in (System.Collections.IEnumerable)value)
                {
                    var placement = (Dictionary<string, object>)item;
                    placements.Add(new PreviewPlacement
                    {
                        PartId = Convert.ToString(placement["partId"]),
                        SheetInstanceId = placement.ContainsKey("sheetInstanceId")
                            ? Convert.ToInt32(placement["sheetInstanceId"])
                            : 0,
                        X = Convert.ToDouble(placement["x"]),
                        Y = Convert.ToDouble(placement["y"]),
                        Rotation = Convert.ToDouble(placement["rotation"])
                    });
                    sheetCount = Math.Max(
                        sheetCount,
                        placements[placements.Count - 1].SheetInstanceId + 1);
                }
            }

            if (root.TryGetValue("sheetsUsed", out value) && value != null)
            {
                foreach (object item in (System.Collections.IEnumerable)value)
                {
                    var sheet = (Dictionary<string, object>)item;
                    if (Convert.ToString(sheet["sheetId"]) == "sheet-1")
                    {
                        sheetCount = Math.Max(
                            sheetCount, Convert.ToInt32(sheet["instancesUsed"]));
                    }
                }
            }

            return new PreviewModel
            {
                SheetWidth = sheetWidth,
                SheetHeight = sheetHeight,
                SheetGap = sheetGap,
                SheetCount = sheetCount,
                Parts = parts,
                Placements = placements
            };
        }

        private static JobRunResult ParseJobResult(string json, double sheetWidth,
            double sheetHeight, List<PreviewPart> parts)
        {
            var root = Json.Deserialize<Dictionary<string, object>>(json);
            bool better = true;
            object statusValue;
            if (root.TryGetValue("status", out statusValue) && statusValue != null)
            {
                var resultStatus = (Dictionary<string, object>)statusValue;
                object betterValue;
                if (resultStatus.TryGetValue("better", out betterValue))
                {
                    better = Convert.ToBoolean(betterValue);
                }
            }
            return new JobRunResult
            {
                Preview = ParsePreview(json, sheetWidth, sheetHeight, parts),
                JobStatus = root.ContainsKey("jobStatus")
                    ? Convert.ToString(root["jobStatus"])
                    : string.Empty,
                UpdatedAt = root.ContainsKey("updatedAt")
                    ? Convert.ToString(root["updatedAt"])
                    : string.Empty,
                StartedAt = root.ContainsKey("startedAt")
                    ? Convert.ToString(root["startedAt"])
                    : string.Empty,
                Fitness = root.ContainsKey("fitness")
                    ? Convert.ToDouble(root["fitness"])
                    : 0,
                Index = root.ContainsKey("index")
                    ? Convert.ToInt32(root["index"])
                    : 0,
                Better = better
            };
        }

        private static string Send(string method, string path, string body)
        {
            using (var request = new HttpRequestMessage(new HttpMethod(method), path))
            {
                request.Headers.Accept.ParseAdd("application/json");
                if (body != null)
                {
                    request.Content = new StringContent(body, Encoding.UTF8, "application/json");
                }

                using (HttpResponseMessage response = Http.SendAsync(request).GetAwaiter().GetResult())
                {
                    string responseBody = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
                    if (!response.IsSuccessStatusCode)
                    {
                        ThrowHttpError(response, responseBody);
                    }

                    return responseBody;
                }
            }
        }

        private static void RequestStop(string jobId)
        {
            string path = "api/v1/jobs/" + jobId + "/stop";
            System.Threading.Tasks.Task.Run(delegate
            {
                try
                {
                    Send("POST", path, null);
                }
                catch
                {
                    // Best effort: the caller also stops the job on exit.
                }
            });
        }

        private static void ThrowHttpError(HttpResponseMessage response, string body)
        {
            throw new HttpRequestException(
                "HTTP " + (int)response.StatusCode + " " + response.ReasonPhrase +
                (string.IsNullOrWhiteSpace(body) ? string.Empty : Environment.NewLine + body));
        }

        private static void ShowError(string title, Exception error)
        {
            MessageBox.Show(
                title + Environment.NewLine + Environment.NewLine + error.Message,
                "CorelDeepnest",
                MessageBoxButtons.OK,
                MessageBoxIcon.Error);
        }

        private sealed class NestingForm : Form
        {
            private readonly ICorelGateway gateway;
            private readonly NumericUpDown width = NumberInput(1000);
            private readonly NumericUpDown height = NumberInput(500);
            private readonly NumericUpDown spacing = NumberInput(0);
            private readonly NumericUpDown rotations = NumberInput(4, 0);
            private readonly NumericUpDown populationSize = NumberInput(10, 0);
            private readonly NumericUpDown mutationRate = NumberInput(10, 0);
            private readonly ComboBox placementType = new ComboBox();
            private readonly NumericUpDown curveTolerance = NumberInput(0.3M, 3);
            private readonly NumericUpDown timeRatio = NumberInput(0.5M, 2);
            private readonly NumericUpDown timeLimitSeconds = NumberInput(0, 0);
            private readonly CheckBox mergeLines = new CheckBox();
            private readonly ToolTip help = new ToolTip();
            private readonly Button run = new Button();
            private readonly Button stop = new Button();
            private readonly Button apply = new Button();
            private readonly Label status = new Label();
            private readonly System.Windows.Forms.Timer tick = new System.Windows.Forms.Timer();
            private readonly PreviewPanel preview = new PreviewPanel();
            private volatile bool stopRequested;
            private JobRunResult completedResult;
            private System.DateTime runStartedUtc;
            private System.DateTime? runServerStartedUtc;
            private int runLimitSeconds;
            private int lastIndex;
            private double lastFitness;
            private int lastPlaced;
            private int lastSheets;

            public NestingForm(ICorelGateway gateway)
            {
                this.gateway = gateway;
                width.Minimum = 0.01M;
                height.Minimum = 0.01M;
                rotations.Minimum = 1;
                rotations.Maximum = 16;
                populationSize.Minimum = 3;
                populationSize.Maximum = 64;
                mutationRate.Minimum = 2;
                mutationRate.Maximum = 64;
                curveTolerance.Minimum = 0.1M;
                curveTolerance.Maximum = 100;
                curveTolerance.Increment = 0.01M;
                timeRatio.Maximum = 1000;
                timeRatio.Increment = 0.1M;
                timeLimitSeconds.Maximum = 86400;
                placementType.DropDownStyle = ComboBoxStyle.DropDownList;
                placementType.Items.AddRange(new object[] { "gravity", "box", "convexhull" });
                placementType.SelectedIndex = 0;
                placementType.Width = 100;
                mergeLines.Text = "Объединять линии";
                mergeLines.Checked = true;
                mergeLines.AutoSize = true;
                mergeLines.Margin = new Padding(8, 23, 8, 0);
                Text = "Deepnest Server — сборка " + BuildInfo.Id;
                Width = 920;
                Height = 680;
                StartPosition = FormStartPosition.CenterParent;

                var fields = new FlowLayoutPanel
                {
                    Dock = DockStyle.Top,
                    AutoSize = true,
                    AutoSizeMode = AutoSizeMode.GrowAndShrink,
                    WrapContents = false,
                    Padding = new Padding(8, 3, 8, 3)
                };

                AddField(fields, "Ширина, мм", width);
                AddField(fields, "Высота, мм", height);
                AddField(fields, "Зазор, мм", spacing);
                AddField(fields, "Вариантов поворота", rotations);

                var advancedFields = new FlowLayoutPanel
                {
                    Dock = DockStyle.Top,
                    AutoSize = true,
                    AutoSizeMode = AutoSizeMode.GrowAndShrink,
                    WrapContents = true,
                    Visible = false,
                    Padding = new Padding(8, 3, 8, 3),
                    BackColor = DrawingColor.FromArgb(242, 242, 242)
                };
                AddField(advancedFields, "Размещение", placementType);
                AddField(advancedFields, "Популяция", populationSize);
                AddField(advancedFields, "Мутация, %", mutationRate);
                AddField(advancedFields, "Точность, мм", curveTolerance);
                AddField(advancedFields, "Вес линий", timeRatio);
                AddField(advancedFields, "Лимит, сек", timeLimitSeconds);
                advancedFields.Controls.Add(mergeLines);

                help.SetToolTip(width, "Ширина листа в миллиметрах.");
                help.SetToolTip(height, "Высота листа в миллиметрах.");
                help.SetToolTip(spacing,
                    "Минимальный зазор между деталями, мм (min separation движка).");
                help.SetToolTip(rotations,
                    "Число равномерных вариантов поворота, а не градусы. 4 = 0, 90, 180 и 270.");
                help.SetToolTip(placementType,
                    "Стратегия размещения: gravity — компактнее по ширине, box — меньше площадь описанного прямоугольника, convexhull — площадь оболочки.");
                help.SetToolTip(populationSize, "Размер популяции генетического алгоритма. Больше — шире поиск.");
                help.SetToolTip(mutationRate, "Вероятность мутации в процентах.");
                help.SetToolTip(curveTolerance,
                    "Точность сглаживания SVG, мм (входная геометрия).");
                help.SetToolTip(timeRatio,
                    "Вес длины общих линий реза в приспособленности: 0 — только материал, 1 — только время резки.");
                help.SetToolTip(timeLimitSeconds,
                    "0 — работать до кнопки «Стоп»; положительное значение — автостоп, сек.");
                help.SetToolTip(mergeLines, "Поощрять раскладки с общими линиями реза.");

                run.Text = "Разложить";
                run.Click += RunClick;

                stop.Text = "Стоп";
                stop.Enabled = false;
                stop.Click += delegate { stopRequested = true; status.Text = "Остановка задачи..."; };

                FormClosing += delegate(object sender, FormClosingEventArgs args)
                {
                    if (stop.Enabled)
                    {
                        args.Cancel = true;
                        stopRequested = true;
                        status.Text = "Остановка задачи перед закрытием...";
                    }
                };

                apply.Text = "Применить в CorelDRAW";
                apply.Enabled = false;
                apply.Click += ApplyClick;

                var advancedButton = new Button { Text = "Настройки ▸" };
                advancedButton.Click += delegate
                {
                    advancedFields.Visible = !advancedFields.Visible;
                    advancedButton.Text = advancedFields.Visible
                        ? "Настройки ▾"
                        : "Настройки ▸";
                };

                status.Text = "Выделите замкнутые векторные объекты и запустите раскладку.";
                status.AutoSize = true;
                status.Padding = new Padding(8, 6, 0, 0);

                tick.Interval = 250;
                tick.Tick += delegate { RefreshRunningStatus(); };

                var actions = new FlowLayoutPanel
                {
                    Dock = DockStyle.Top,
                    AutoSize = true,
                    AutoSizeMode = AutoSizeMode.GrowAndShrink,
                    WrapContents = false,
                    Padding = new Padding(8, 4, 8, 4)
                };
                actions.Controls.Add(run);
                actions.Controls.Add(stop);
                actions.Controls.Add(apply);
                actions.Controls.Add(advancedButton);
                actions.Controls.Add(status);

                Controls.Add(preview);
                Controls.Add(actions);
                Controls.Add(advancedFields);
                Controls.Add(fields);

                LoadSettings();
                FormClosed += delegate { SaveSettings(); };
            }

            private void RunClick(object sender, EventArgs e)
            {
                run.Enabled = false;
                stop.Enabled = true;
                apply.Enabled = false;
                stopRequested = false;
                completedResult = null;
                preview.Model = null;
                runStartedUtc = System.DateTime.UtcNow;
                runServerStartedUtc = null;
                runLimitSeconds = 0;
                lastIndex = 0;
                lastFitness = 0;
                lastPlaced = 0;
                lastSheets = 0;
                tick.Start();
                status.Text = "Создание задачи...";
                System.Windows.Forms.Application.DoEvents();

                NestingOptions options = null;
                string requestJson = null;
                List<PreviewPart> previewParts = null;
                try
                {
                    // The Corel SVG export touches COM and must run on the UI
                    // thread; the job HTTP polling runs off it.
                    options = ReadOptions();
                    runLimitSeconds = options.TimeLimitSeconds;
                    SaveSettings();
                    requestJson = Json.Serialize(
                        BuildRequest(gateway, options, out previewParts));
                }
                catch (Exception error)
                {
                    status.Text = "Не удалось подготовить задачу.";
                    ShowError("Не удалось подготовить задачу", error);
                    run.Enabled = true;
                    stop.Enabled = false;
                    tick.Stop();
                    return;
                }

                System.Threading.Tasks.Task.Run(delegate
                {
                    try
                    {
                        JobRunResult jobResult = RunJob(
                            requestJson,
                            options,
                            previewParts,
                            delegate(string jobId)
                            {
                                Post((MethodInvoker)delegate
                                {
                                    status.Text = "Задача " + jobId + " выполняется...";
                                });
                            },
                            delegate(JobRunResult result)
                            {
                                Post((MethodInvoker)delegate { UpdateRunningResult(result); });
                            },
                            delegate { return stopRequested; });
                        Post((MethodInvoker)delegate { FinishRun(jobResult, null); });
                    }
                    catch (Exception error)
                    {
                        Post((MethodInvoker)delegate { FinishRun(null, error); });
                    }
                });
            }

            private void Post(MethodInvoker action)
            {
                try
                {
                    if (IsHandleCreated && !IsDisposed)
                    {
                        BeginInvoke(action);
                    }
                }
                catch
                {
                    // The form is closing or already disposed; drop the update.
                }
            }

            private void FinishRun(JobRunResult jobResult, Exception error)
            {
                if (error != null)
                {
                    status.Text = error is OperationCanceledException
                        ? "Задача остановлена."
                        : "Ошибка задачи.";
                    if (!(error is OperationCanceledException))
                    {
                        ShowError("Ошибка задачи Deepnest", error);
                    }
                }
                else if (jobResult != null)
                {
                    preview.Model = jobResult.Preview;
                    completedResult = jobResult;
                    apply.Enabled = jobResult.Preview.Placements.Count > 0;
                    status.Text = "Принят лучший вариант: деталей — " +
                        jobResult.Preview.Placements.Count + ", листов — " +
                        jobResult.Preview.SheetCount + ".";
                }

                run.Enabled = true;
                stop.Enabled = false;
                tick.Stop();
            }

            private void UpdateRunningResult(JobRunResult jobResult)
            {
                if (!string.IsNullOrEmpty(jobResult.StartedAt))
                {
                    System.DateTime serverStart;
                    if (System.DateTime.TryParse(
                        jobResult.StartedAt,
                        System.Globalization.CultureInfo.InvariantCulture,
                        System.Globalization.DateTimeStyles.AdjustToUniversal |
                            System.Globalization.DateTimeStyles.AssumeUniversal,
                        out serverStart))
                    {
                        runServerStartedUtc = serverStart;
                    }
                }
                lastIndex = jobResult.Index;
                lastFitness = jobResult.Fitness;
                lastPlaced = jobResult.Preview.Placements.Count;
                lastSheets = jobResult.Preview.SheetCount;
                preview.Model = jobResult.Preview;
                completedResult = jobResult;
                RefreshRunningStatus();
            }

            private void RefreshRunningStatus()
            {
                System.DateTime baseUtc = runServerStartedUtc ?? runStartedUtc;
                System.TimeSpan span = System.DateTime.UtcNow - baseUtc;
                if (span < System.TimeSpan.Zero)
                {
                    span = System.TimeSpan.Zero;
                }
                int seconds = (int)System.Math.Floor(span.TotalSeconds);
                status.Text = "вариант " + lastIndex +
                    " · fitness " + lastFitness.ToString("0.###") +
                    " · деталей " + lastPlaced +
                    " · листов " + lastSheets + " · " +
                    (runLimitSeconds > 0
                        ? seconds + "/" + runLimitSeconds + " с"
                        : FormatSeconds(seconds));
            }

            private static string FormatSeconds(int seconds)
            {
                return seconds < 60
                    ? seconds + " с"
                    : (seconds / 60) + ":" + (seconds % 60).ToString("00");
            }

            private void ApplyClick(object sender, EventArgs e)
            {
                if (completedResult == null || completedResult.Preview == null)
                {
                    return;
                }

                apply.Enabled = false;
                try
                {
                    var placements = new List<object>();
                    foreach (PreviewPlacement placement in completedResult.Preview.Placements)
                    {
                        placements.Add(new Dictionary<string, object>
                        {
                            { "partId", placement.PartId },
                            { "sheetInstanceId", placement.SheetInstanceId },
                            { "x", placement.X },
                            { "y", placement.Y },
                            { "rotation", placement.Rotation }
                        });
                    }

                    string response = gateway.Invoke(
                        "ApplyPlacements",
                        Json.Serialize(new Dictionary<string, object>
                        {
                            { "sheetWidth", completedResult.Preview.SheetWidth },
                            { "sheetHeight", completedResult.Preview.SheetHeight },
                            { "sheetGap", completedResult.Preview.SheetGap },
                            { "placements", placements.ToArray() }
                        }));
                    var applied = Json.Deserialize<Dictionary<string, object>>(response);
                    status.Text = "Применено деталей — " + Convert.ToInt32(applied["applied"]) +
                        ", листов — " + Convert.ToInt32(applied["sheets"]) +
                        ", слой " + Convert.ToString(applied["layer"]) + ".";
                }
                catch (Exception error)
                {
                    status.Text = "Не удалось применить размещение.";
                    ShowError("Ошибка применения размещения", error);
                    apply.Enabled = true;
                }
            }

            private NestingOptions ReadOptions()
            {
                return new NestingOptions
                {
                    SheetWidth = Convert.ToDouble(width.Value),
                    SheetHeight = Convert.ToDouble(height.Value),
                    Spacing = Convert.ToDouble(spacing.Value),
                    Rotations = Convert.ToInt32(rotations.Value),
                    PopulationSize = Convert.ToInt32(populationSize.Value),
                    MutationRate = Convert.ToInt32(mutationRate.Value),
                    PlacementType = Convert.ToString(placementType.SelectedItem),
                    MergeLines = mergeLines.Checked,
                    CurveTolerance = Convert.ToDouble(curveTolerance.Value),
                    TimeRatio = Convert.ToDouble(timeRatio.Value),
                    TimeLimitSeconds = Convert.ToInt32(timeLimitSeconds.Value)
                };
            }

            private void LoadSettings()
            {
                try
                {
                    string path = SettingsPath();
                    if (!File.Exists(path))
                    {
                        return;
                    }

                    var values = Json.Deserialize<Dictionary<string, object>>(
                        File.ReadAllText(path));
                    SetNumber(values, "sheetWidth", width);
                    SetNumber(values, "sheetHeight", height);
                    SetNumber(values, "spacing", spacing);
                    SetNumber(values, "rotations", rotations);
                    SetNumber(values, "populationSize", populationSize);
                    SetNumber(values, "mutationRate", mutationRate);
                    SetNumber(values, "curveTolerance", curveTolerance);
                    SetNumber(values, "timeRatio", timeRatio);
                    SetNumber(values, "timeLimitSeconds", timeLimitSeconds);

                    SetCombo(values, "placementType", placementType);
                    object mergeValue;
                    if (values.TryGetValue("mergeLines", out mergeValue) &&
                        mergeValue != null)
                    {
                        mergeLines.Checked = Convert.ToBoolean(mergeValue);
                    }
                }
                catch
                {
                    // Invalid or obsolete settings should not prevent the addon opening.
                }
            }

            private void SaveSettings()
            {
                try
                {
                    NestingOptions options = ReadOptions();
                    string path = SettingsPath();
                    Directory.CreateDirectory(Path.GetDirectoryName(path));
                    File.WriteAllText(path, Json.Serialize(new Dictionary<string, object>
                    {
                        { "sheetWidth", options.SheetWidth },
                        { "sheetHeight", options.SheetHeight },
                        { "spacing", options.Spacing },
                        { "rotations", options.Rotations },
                        { "populationSize", options.PopulationSize },
                        { "mutationRate", options.MutationRate },
                        { "placementType", options.PlacementType },
                        { "mergeLines", options.MergeLines },
                        { "curveTolerance", options.CurveTolerance },
                        { "timeRatio", options.TimeRatio },
                        { "timeLimitSeconds", options.TimeLimitSeconds }
                    }));
                }
                catch
                {
                    // Nesting remains usable even if local settings cannot be saved.
                }
            }

            private static void SetNumber(Dictionary<string, object> values,
                string name, NumericUpDown control)
            {
                object value;
                if (!values.TryGetValue(name, out value) || value == null)
                {
                    return;
                }

                decimal number = Convert.ToDecimal(value);
                control.Value = Math.Min(control.Maximum,
                    Math.Max(control.Minimum, number));
            }

            private static void SetCombo(Dictionary<string, object> values,
                string name, ComboBox control)
            {
                object value;
                if (!values.TryGetValue(name, out value) || value == null)
                {
                    return;
                }

                int index = control.Items.IndexOf(Convert.ToString(value));
                if (index >= 0)
                {
                    control.SelectedIndex = index;
                }
            }

            private static string SettingsPath()
            {
                return Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                    "CorelDeepnest",
                    "settings.json");
            }

            private static NumericUpDown NumberInput(decimal value, int decimals = 2)
            {
                return new NumericUpDown
                {
                    DecimalPlaces = decimals,
                    Minimum = 0,
                    Maximum = 1000000,
                    Value = value,
                    Width = 90
                };
            }

            private static void AddField(FlowLayoutPanel panel, string label,
                System.Windows.Forms.Control control)
            {
                var field = new TableLayoutPanel
                {
                    AutoSize = true,
                    AutoSizeMode = AutoSizeMode.GrowAndShrink,
                    RowCount = 2,
                    ColumnCount = 1,
                    Margin = new Padding(4, 0, 8, 0)
                };
                field.Controls.Add(new Label
                {
                    Text = label,
                    AutoSize = true
                }, 0, 0);
                field.Controls.Add(control, 0, 1);
                panel.Controls.Add(field);
            }
        }

        private sealed class PreviewPanel : Panel
        {
            private const float MarginSize = 24f;
            private const float SheetGapPx = 16f;

            private PreviewModel model;

            public PreviewModel Model
            {
                get { return model; }
                set { model = value; Invalidate(); }
            }

            public PreviewPanel()
            {
                Dock = DockStyle.Fill;
                BackColor = DrawingColor.FromArgb(36, 39, 44);
                DoubleBuffered = true;
                AutoScroll = true;
            }

            protected override void OnResize(EventArgs e)
            {
                base.OnResize(e);
                Invalidate();
            }

            protected override void OnPaint(PaintEventArgs e)
            {
                base.OnPaint(e);
                e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;

                if (model == null || model.SheetWidth <= 0 || model.SheetHeight <= 0)
                {
                    using (Brush textBrush = new SolidBrush(DrawingColor.Gainsboro))
                    {
                        e.Graphics.DrawString("Здесь появится предпросмотр раскладки.",
                            Font, textBrush, 16, 16);
                    }
                    return;
                }

                int sheetCount = Math.Max(1, model.SheetCount);

                // Each sheet is stretched to the full available height and the
                // sheets are laid out in a single row; the panel scrolls
                // horizontally when they do not all fit.
                float availableHeight = Math.Max(1f, ClientSize.Height - MarginSize * 2);
                float scale = availableHeight / (float)model.SheetHeight;
                if (!(scale > 0f))
                {
                    scale = 1f;
                }
                float sheetHeight = (float)model.SheetHeight * scale;
                float sheetWidth = (float)model.SheetWidth * scale;

                var origins = new System.Drawing.PointF[sheetCount];
                for (int sheetIndex = 0; sheetIndex < sheetCount; sheetIndex++)
                {
                    float x = MarginSize + sheetIndex * (sheetWidth + SheetGapPx);
                    origins[sheetIndex] = new System.Drawing.PointF(x, MarginSize);
                }
                float contentWidth = MarginSize * 2 + sheetCount * sheetWidth +
                    (sheetCount - 1) * SheetGapPx;

                var desired = new System.Drawing.Size((int)Math.Ceiling(contentWidth), 0);
                if (AutoScrollMinSize != desired)
                {
                    AutoScrollMinSize = desired;
                }

                // Draw in scrollable (virtual) coordinates.
                e.Graphics.TranslateTransform(AutoScrollPosition.X, AutoScrollPosition.Y);

                using (Brush sheetBrush = new SolidBrush(DrawingColor.WhiteSmoke))
                using (Pen sheetPen = new Pen(DrawingColor.Silver, 2))
                using (Brush sheetLabelBrush = new SolidBrush(DrawingColor.DimGray))
                {
                    for (int sheetIndex = 0; sheetIndex < sheetCount; sheetIndex++)
                    {
                        System.Drawing.PointF origin = origins[sheetIndex];
                        e.Graphics.FillRectangle(
                            sheetBrush, origin.X, origin.Y, sheetWidth, sheetHeight);
                        e.Graphics.DrawRectangle(
                            sheetPen, origin.X, origin.Y, sheetWidth, sheetHeight);
                        e.Graphics.DrawString(
                            "Лист " + (sheetIndex + 1), Font, sheetLabelBrush,
                            origin.X + 6, origin.Y + 6);
                    }
                }

                DrawingColor[] colors =
                {
                    DrawingColor.FromArgb(58, 41, 128, 185),
                    DrawingColor.FromArgb(58, 39, 174, 96),
                    DrawingColor.FromArgb(58, 243, 156, 18),
                    DrawingColor.FromArgb(58, 142, 68, 173),
                    DrawingColor.FromArgb(58, 192, 57, 43)
                };

                for (int placementIndex = 0;
                    placementIndex < model.Placements.Count; placementIndex++)
                {
                    PreviewPlacement placement = model.Placements[placementIndex];
                    PreviewPart part = model.Parts.Find(
                        delegate(PreviewPart candidate) { return candidate.Id == placement.PartId; });
                    if (part == null || part.Contours.Count == 0)
                    {
                        continue;
                    }

                    int sheetSlot = placement.SheetInstanceId >= 0 &&
                        placement.SheetInstanceId < origins.Length
                        ? placement.SheetInstanceId
                        : 0;
                    float placementSheetX = origins[sheetSlot].X;
                    float placementSheetY = origins[sheetSlot].Y;

                    double radians = placement.Rotation * Math.PI / 180.0;
                    double cos = Math.Cos(radians);
                    double sin = Math.Sin(radians);
                    DrawingColor color = colors[placementIndex % colors.Length];
                    using (Brush fill = new SolidBrush(color))
                    using (Pen outline = new Pen(
                        DrawingColor.FromArgb(235, color.R, color.G, color.B), 1.8f))
                    using (Brush labelBrush = new SolidBrush(DrawingColor.Black))
                    using (var path = new GraphicsPath(FillMode.Alternate))
                    using (var labelFormat = new StringFormat
                    {
                        Alignment = StringAlignment.Center,
                        LineAlignment = StringAlignment.Center
                    })
                    {
                        foreach (PreviewContour contour in part.Contours)
                        {
                            if (contour.Points.Count < 3)
                            {
                                continue;
                            }
                            path.AddPolygon(TransformPolygon(
                                contour.Points, cos, sin, placement, placementSheetX,
                                placementSheetY, sheetHeight, scale));
                            foreach (List<GeometryPoint> hole in contour.Holes)
                            {
                                path.AddPolygon(TransformPolygon(
                                    hole, cos, sin, placement, placementSheetX,
                                    placementSheetY, sheetHeight, scale));
                            }
                        }
                        if (path.PointCount == 0)
                        {
                            continue;
                        }
                        e.Graphics.FillPath(fill, path);
                        e.Graphics.DrawPath(outline, path);
                        RectangleF bounds = path.GetBounds();
                        string label = CompactPartLabel(placement.PartId);
                        e.Graphics.DrawString(
                            label, Font, labelBrush, bounds, labelFormat);
                    }
                }
            }

            private static System.Drawing.PointF[] TransformPolygon(
                List<GeometryPoint> points, double cos, double sin,
                PreviewPlacement placement, float placementSheetX,
                float originY, float sheetHeight, float scale)
            {
                var polygon = new System.Drawing.PointF[points.Count];
                for (int index = 0; index < points.Count; index++)
                {
                    GeometryPoint point = points[index];
                    double worldX = point.X * cos - point.Y * sin + placement.X;
                    double worldY = point.X * sin + point.Y * cos + placement.Y;
                    polygon[index] = new System.Drawing.PointF(
                        placementSheetX + (float)worldX * scale,
                        originY + sheetHeight - (float)worldY * scale);
                }
                return polygon;
            }

            private static string CompactPartLabel(string partId)
            {
                const string prefix = "part-";
                return partId != null && partId.StartsWith(
                    prefix, StringComparison.OrdinalIgnoreCase)
                    ? "#" + partId.Substring(prefix.Length).Replace("#", ".")
                    : partId;
            }
        }
    }
}
