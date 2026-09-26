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
            public string Json;
            public PreviewModel Preview;
            public string JobStatus;
            public string UpdatedAt;
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
            public int Threads;
            public int CorelCurvePrecision;
            public string PlacementType;
            public bool MergeLines;
            public double CurveTolerance;
            public double TimeRatio;
            public int TimeLimitSeconds;
            public bool UseSvgInput;
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

            throw new InvalidOperationException("Unknown CorelDeepnest command: " + command);
        }

        private static void TestDeepnestConnection()
        {
            try
            {
                MessageBox.Show(
                    "Deepnest Server connection OK" + Environment.NewLine + Environment.NewLine +
                    Send("GET", "health", null),
                    "CorelDeepnest",
                    MessageBoxButtons.OK,
                    MessageBoxIcon.Information);
            }
            catch (Exception error)
            {
                ShowError("Deepnest Server connection failed", error);
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
                ShowError("Deepnest window failed", error);
            }
        }

        private static JobRunResult RunJob(ICorelGateway gateway, NestingOptions options,
            Action<string> jobCreated, Action<JobRunResult> resultUpdated,
            Func<bool> stopRequested)
        {
            string jobId = null;
            List<PreviewPart> previewParts;
            JobRunResult latestResult = null;
            string latestUpdate = null;
            bool stopSent = false;

            try
            {
                string requestJson = Json.Serialize(BuildRequest(
                    gateway, options, out previewParts));
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
                        Send("POST", "api/v1/jobs/" + jobId + "/stop", null);
                        stopSent = true;
                        if (latestResult == null)
                        {
                            throw new OperationCanceledException(
                                "The nesting job was stopped before it produced a placement.");
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

                    System.Windows.Forms.Application.DoEvents();
                    Thread.Sleep(200);
                }
            }
            finally
            {
                if (!string.IsNullOrEmpty(jobId) && !stopSent)
                {
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
            if (options.UseSvgInput)
            {
                return BuildSvgRequest(gateway, options, out previewParts);
            }

            var selection = Json.Deserialize<Dictionary<string, object>>(
                gateway.CaptureSelectionJson(options.CorelCurvePrecision));
            var parts = new List<object>();
            previewParts = new List<PreviewPart>();
            foreach (object partValue in (System.Collections.IEnumerable)selection["parts"])
            {
                var capturedPart = (Dictionary<string, object>)partValue;
                string partId = Convert.ToString(capturedPart["id"]);
                var polygonTree = (Dictionary<string, object>)capturedPart["polygontree"];

                parts.Add(new Dictionary<string, object>
                {
                    { "id", partId },
                    { "quantity", 1 },
                    { "polygontree", polygonTree }
                });
                previewParts.Add(ParsePreviewPart(partId, polygonTree));
            }

            object[] sheetPoints =
            {
                Point(0, 0),
                Point(options.SheetWidth, 0),
                Point(options.SheetWidth, options.SheetHeight),
                Point(0, options.SheetHeight)
            };

            var sheet = new Dictionary<string, object>
            {
                { "id", "sheet-1" },
                { "quantity", 1 },
                { "mode", "auto" },
                { "polygontree", Polygon(sheetPoints) }
            };

            var input = new Dictionary<string, object>
            {
                { "format", "geometry" },
                { "units", "mm" },
                { "sheets", new object[] { sheet } },
                { "parts", parts.ToArray() }
            };

            var config = new Dictionary<string, object>
            {
                { "spacing", options.Spacing },
                { "rotations", options.Rotations },
                { "populationSize", options.PopulationSize },
                { "mutationRate", options.MutationRate },
                { "threads", options.Threads },
                { "placementType", options.PlacementType },
                { "mergeLines", options.MergeLines },
                { "curveTolerance", options.CurveTolerance },
                { "timeRatio", options.TimeRatio }
            };

            var request = new Dictionary<string, object>
            {
                { "input", input },
                { "config", config }
            };
            if (options.TimeLimitSeconds > 0)
            {
                request.Add("execution", new Dictionary<string, object>
                {
                    { "timeLimitMs", options.TimeLimitSeconds * 1000 }
                });
            }
            return request;
        }

        private static Dictionary<string, object> BuildSvgRequest(
            ICorelGateway gateway, NestingOptions options,
            out List<PreviewPart> previewParts)
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
                    { "quantity", 1 }
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
                                { "data", binSvg }
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
                { "threads", options.Threads },
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

        private static PreviewPart ParsePreviewPart(
            string partId, Dictionary<string, object> polygonTree)
        {
            var holes = new List<List<GeometryPoint>>();
            CollectChildContours(polygonTree, holes);
            return new PreviewPart
            {
                Id = partId,
                Points = ParseGeometryPoints(polygonTree["points"]),
                Holes = holes
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

        private static Dictionary<string, object> Point(double x, double y)
        {
            return new Dictionary<string, object> { { "x", x }, { "y", y } };
        }

        private static Dictionary<string, object> Polygon(object[] points)
        {
            return new Dictionary<string, object>
            {
                { "points", points },
                { "children", new object[0] }
            };
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
                    parts.Add(ParsePreviewPart(
                        Convert.ToString(part["id"]),
                        (Dictionary<string, object>)part["polygontree"]));
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
                Json = json,
                Preview = ParsePreview(json, sheetWidth, sheetHeight, parts),
                JobStatus = root.ContainsKey("jobStatus")
                    ? Convert.ToString(root["jobStatus"])
                    : string.Empty,
                UpdatedAt = root.ContainsKey("updatedAt")
                    ? Convert.ToString(root["updatedAt"])
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
            private readonly NumericUpDown threads = NumberInput(4, 0);
            private readonly NumericUpDown corelCurvePrecision = NumberInput(50, 0);
            private readonly NumericUpDown curveTolerance = NumberInput(0.72M, 3);
            private readonly NumericUpDown timeRatio = NumberInput(0.5M, 2);
            private readonly NumericUpDown timeLimitSeconds = NumberInput(0, 0);
            private readonly ComboBox placementType = new ComboBox();
            private readonly ComboBox inputFormat = new ComboBox();
            private readonly CheckBox mergeLines = new CheckBox();
            private readonly ToolTip help = new ToolTip();
            private readonly TextBox result = new TextBox();
            private readonly Button run = new Button();
            private readonly Button stop = new Button();
            private readonly Button apply = new Button();
            private readonly Label status = new Label();
            private readonly PreviewPanel preview = new PreviewPanel();
            private bool stopRequested;
            private JobRunResult completedResult;

            public NestingForm(ICorelGateway gateway)
            {
                this.gateway = gateway;
                width.Minimum = 0.01M;
                height.Minimum = 0.01M;
                rotations.Minimum = 1;
                populationSize.Minimum = 3;
                populationSize.Maximum = 1000;
                mutationRate.Minimum = 1;
                mutationRate.Maximum = 100;
                threads.Minimum = 1;
                threads.Maximum = 8;
                corelCurvePrecision.Minimum = 1;
                corelCurvePrecision.Maximum = 100;
                curveTolerance.Minimum = 0.001M;
                curveTolerance.Maximum = 100;
                curveTolerance.Increment = 0.01M;
                timeRatio.Maximum = 1000;
                timeRatio.Increment = 0.1M;
                timeLimitSeconds.Maximum = 86400;
                placementType.DropDownStyle = ComboBoxStyle.DropDownList;
                placementType.Items.AddRange(new object[]
                {
                    "gravity", "box", "convexhull"
                });
                placementType.SelectedIndex = 0;
                placementType.Width = 100;
                inputFormat.DropDownStyle = ComboBoxStyle.DropDownList;
                inputFormat.Items.AddRange(new object[]
                {
                    "Geometry (COM)", "SVG (experimental)"
                });
                inputFormat.SelectedIndex = 1;
                inputFormat.Width = 130;
                mergeLines.Text = "Merge lines";
                mergeLines.Checked = true;
                mergeLines.AutoSize = true;
                mergeLines.Margin = new Padding(8, 23, 8, 0);
                Text = "Deepnest Server — runtime " + BuildInfo.Id;
                Width = 920;
                Height = 680;
                StartPosition = FormStartPosition.CenterParent;

                var fields = new FlowLayoutPanel
                {
                    Dock = DockStyle.Top,
                    Height = 58,
                    WrapContents = false,
                    Padding = new Padding(8, 3, 8, 3)
                };

                AddField(fields, "Width, mm", width);
                AddField(fields, "Height, mm", height);
                AddField(fields, "Spacing, mm", spacing);
                AddField(fields, "Rotations", rotations);

                var advancedFields = new FlowLayoutPanel
                {
                    Dock = DockStyle.Top,
                    Height = 58,
                    WrapContents = false,
                    AutoScroll = true,
                    Visible = false,
                    Padding = new Padding(8, 3, 8, 3),
                    BackColor = DrawingColor.FromArgb(242, 242, 242)
                };
                AddField(advancedFields, "Placement", placementType);
                AddField(advancedFields, "Input", inputFormat);
                AddField(advancedFields, "Population", populationSize);
                AddField(advancedFields, "Mutation, %", mutationRate);
                AddField(advancedFields, "Threads", threads);
                AddField(advancedFields, "Curve detail", corelCurvePrecision);
                AddField(advancedFields, "Tolerance, mm", curveTolerance);
                AddField(advancedFields, "Line weight", timeRatio);
                AddField(advancedFields, "Limit, sec", timeLimitSeconds);
                advancedFields.Controls.Add(mergeLines);

                help.SetToolTip(placementType,
                    "gravity favors compact width; box minimizes bounding-box area; convexhull minimizes hull area.");
                help.SetToolTip(inputFormat,
                    "SVG lets Corel export rendered geometry and lets the server flatten it. Geometry uses direct COM extraction.");
                help.SetToolTip(populationSize, "Genetic population size. Larger values explore more candidates.");
                help.SetToolTip(mutationRate, "Mutation probability in percent.");
                help.SetToolTip(threads, "Worker count, from 1 to 8.");
                help.SetToolTip(corelCurvePrecision,
                    "Corel curve-to-polyline precision, from 1 to 100. Higher values create more points.");
                help.SetToolTip(curveTolerance, "Geometry tolerance in millimeters.");
                help.SetToolTip(timeRatio, "Weight of shared cutting-line length in fitness.");
                help.SetToolTip(timeLimitSeconds, "0 means run until Stop; a positive value stops automatically.");
                help.SetToolTip(mergeLines, "Reward placements that share compatible cutting lines.");

                run.Text = "Run nesting";
                run.Click += RunClick;

                stop.Text = "Stop";
                stop.Enabled = false;
                stop.Click += delegate { stopRequested = true; status.Text = "Stopping job..."; };

                FormClosing += delegate(object sender, FormClosingEventArgs args)
                {
                    if (stop.Enabled)
                    {
                        args.Cancel = true;
                        stopRequested = true;
                        status.Text = "Stopping job before closing...";
                    }
                };

                apply.Text = "Apply to CorelDRAW";
                apply.Enabled = false;
                apply.Click += ApplyClick;

                var advancedButton = new Button { Text = "Settings ▸" };
                advancedButton.Click += delegate
                {
                    advancedFields.Visible = !advancedFields.Visible;
                    advancedButton.Text = advancedFields.Visible
                        ? "Settings ▾"
                        : "Settings ▸";
                };

                status.Text = "Select closed vector shapes, then run nesting.";
                status.AutoSize = true;
                status.Padding = new Padding(8, 8, 8, 8);

                var actions = new FlowLayoutPanel
                {
                    Dock = DockStyle.Top,
                    Height = 42,
                    Padding = new Padding(8, 4, 8, 4)
                };
                actions.Controls.Add(run);
                actions.Controls.Add(stop);
                actions.Controls.Add(apply);
                actions.Controls.Add(advancedButton);
                actions.Controls.Add(status);

                result.Dock = DockStyle.Fill;
                result.Multiline = true;
                result.ScrollBars = ScrollBars.Both;
                result.WordWrap = false;
                result.ReadOnly = true;

                var split = new SplitContainer
                {
                    Dock = DockStyle.Fill,
                    Orientation = Orientation.Horizontal,
                    SplitterDistance = 380
                };
                split.Panel1.Controls.Add(preview);
                split.Panel2.Controls.Add(result);

                Controls.Add(split);
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
                result.Text = "Creating nesting job...";
                status.Text = "Creating nesting job...";
                System.Windows.Forms.Application.DoEvents();

                try
                {
                    NestingOptions options = ReadOptions();
                    SaveSettings();
                    JobRunResult jobResult = RunJob(
                        gateway,
                        options,
                        delegate(string jobId) { status.Text = "Job " + jobId + " is running..."; },
                        UpdateRunningResult,
                        delegate { return stopRequested; });
                    result.Text = jobResult.Json;
                    preview.Model = jobResult.Preview;
                    completedResult = jobResult;
                    apply.Enabled = jobResult.Preview.Placements.Count > 0;
                    status.Text = "Best placement accepted: " +
                        jobResult.Preview.Placements.Count + " part(s) on " +
                        jobResult.Preview.SheetCount + " sheet(s).";
                }
                catch (Exception error)
                {
                    result.Text = "Deepnest job failed:" + Environment.NewLine + error.Message;
                    status.Text = error is OperationCanceledException ? "Job stopped." : "Job failed.";
                }
                finally
                {
                    run.Enabled = true;
                    stop.Enabled = false;
                }
            }

            private void UpdateRunningResult(JobRunResult jobResult)
            {
                result.Text = jobResult.Json;
                preview.Model = jobResult.Preview;
                completedResult = jobResult;
                status.Text = "Searching: best candidate " + jobResult.Index +
                    ", fitness " + jobResult.Fitness.ToString("0.###") +
                    ", " + jobResult.Preview.Placements.Count + " part(s) on " +
                    jobResult.Preview.SheetCount + " sheet(s). Press Stop to accept.";
                System.Windows.Forms.Application.DoEvents();
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
                    status.Text = "Applied " + Convert.ToInt32(applied["applied"]) +
                        " part(s) on " + Convert.ToInt32(applied["sheets"]) +
                        " sheet(s), layer " + Convert.ToString(applied["layer"]) + ".";
                }
                catch (Exception error)
                {
                    status.Text = "Could not apply placements.";
                    ShowError("Applying placements failed", error);
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
                    Threads = Convert.ToInt32(threads.Value),
                    CorelCurvePrecision = Convert.ToInt32(corelCurvePrecision.Value),
                    PlacementType = Convert.ToString(placementType.SelectedItem),
                    MergeLines = mergeLines.Checked,
                    CurveTolerance = Convert.ToDouble(curveTolerance.Value),
                    TimeRatio = Convert.ToDouble(timeRatio.Value),
                    TimeLimitSeconds = Convert.ToInt32(timeLimitSeconds.Value),
                    UseSvgInput = inputFormat.SelectedIndex == 1
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
                    SetNumber(values, "threads", threads);
                    SetNumber(values, "corelCurvePrecision", corelCurvePrecision);
                    SetNumber(values, "curveTolerance", curveTolerance);
                    SetNumber(values, "timeRatio", timeRatio);
                    SetNumber(values, "timeLimitSeconds", timeLimitSeconds);

                    object value;
                    if (values.TryGetValue("placementType", out value))
                    {
                        string selected = Convert.ToString(value);
                        int index = placementType.Items.IndexOf(selected);
                        if (index >= 0)
                        {
                            placementType.SelectedIndex = index;
                        }
                    }
                    if (values.TryGetValue("mergeLines", out value))
                    {
                        mergeLines.Checked = Convert.ToBoolean(value);
                    }
                    if (values.TryGetValue("inputFormat", out value))
                    {
                        inputFormat.SelectedIndex = Convert.ToString(value) == "svg" ? 1 : 0;
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
                        { "threads", options.Threads },
                        { "corelCurvePrecision", options.CorelCurvePrecision },
                        { "placementType", options.PlacementType },
                        { "mergeLines", options.MergeLines },
                        { "curveTolerance", options.CurveTolerance },
                        { "timeRatio", options.TimeRatio },
                        { "timeLimitSeconds", options.TimeLimitSeconds },
                        { "inputFormat", options.UseSvgInput ? "svg" : "geometry" }
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
            }

            protected override void OnPaint(PaintEventArgs e)
            {
                base.OnPaint(e);
                e.Graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;

                if (model == null || model.SheetWidth <= 0 || model.SheetHeight <= 0)
                {
                    using (Brush textBrush = new SolidBrush(DrawingColor.Gainsboro))
                    {
                        e.Graphics.DrawString("The placement preview will appear here.",
                            Font, textBrush, 16, 16);
                    }
                    return;
                }

                const float margin = 24;
                int sheetCount = Math.Max(1, model.SheetCount);
                double totalModelWidth = model.SheetWidth * sheetCount +
                    model.SheetGap * (sheetCount - 1);
                float scale = Math.Min(
                    (ClientSize.Width - margin * 2) / (float)totalModelWidth,
                    (ClientSize.Height - margin * 2) / (float)model.SheetHeight);
                float sheetWidth = (float)model.SheetWidth * scale;
                float sheetHeight = (float)model.SheetHeight * scale;
                float sheetGap = (float)model.SheetGap * scale;
                float totalWidth = sheetWidth * sheetCount + sheetGap * (sheetCount - 1);
                float originX = (ClientSize.Width - totalWidth) / 2;
                float originY = (ClientSize.Height - sheetHeight) / 2;

                using (Brush sheetBrush = new SolidBrush(DrawingColor.WhiteSmoke))
                using (Pen sheetPen = new Pen(DrawingColor.Silver, 2))
                using (Brush sheetLabelBrush = new SolidBrush(DrawingColor.DimGray))
                {
                    for (int sheetIndex = 0; sheetIndex < sheetCount; sheetIndex++)
                    {
                        float sheetX = originX + sheetIndex * (sheetWidth + sheetGap);
                        e.Graphics.FillRectangle(
                            sheetBrush, sheetX, originY, sheetWidth, sheetHeight);
                        e.Graphics.DrawRectangle(
                            sheetPen, sheetX, originY, sheetWidth, sheetHeight);
                        e.Graphics.DrawString(
                            "Sheet " + (sheetIndex + 1), Font, sheetLabelBrush,
                            sheetX + 6, originY + 6);
                    }
                }

                DrawingColor[] colors =
                {
                    DrawingColor.FromArgb(170, 41, 128, 185),
                    DrawingColor.FromArgb(170, 39, 174, 96),
                    DrawingColor.FromArgb(170, 243, 156, 18),
                    DrawingColor.FromArgb(170, 142, 68, 173),
                    DrawingColor.FromArgb(170, 192, 57, 43)
                };

                for (int placementIndex = 0;
                    placementIndex < model.Placements.Count; placementIndex++)
                {
                    PreviewPlacement placement = model.Placements[placementIndex];
                    PreviewPart part = model.Parts.Find(
                        delegate(PreviewPart candidate) { return candidate.Id == placement.PartId; });
                    if (part == null || part.Points.Count < 3)
                    {
                        continue;
                    }

                    double radians = placement.Rotation * Math.PI / 180.0;
                    double cos = Math.Cos(radians);
                    double sin = Math.Sin(radians);
                    float placementSheetX = originX + placement.SheetInstanceId *
                        (sheetWidth + sheetGap);
                    System.Drawing.PointF[] polygon = TransformPolygon(
                        part.Points, cos, sin, placement, placementSheetX,
                        originY, sheetHeight, scale);

                    DrawingColor color = colors[placementIndex % colors.Length];
                    using (Brush fill = new SolidBrush(color))
                    using (Pen outline = new Pen(DrawingColor.FromArgb(220, color), 1.5f))
                    using (Brush labelBrush = new SolidBrush(DrawingColor.Black))
                    using (var path = new GraphicsPath(FillMode.Alternate))
                    using (var labelFormat = new StringFormat
                    {
                        Alignment = StringAlignment.Center,
                        LineAlignment = StringAlignment.Center
                    })
                    {
                        path.AddPolygon(polygon);
                        foreach (List<GeometryPoint> hole in part.Holes)
                        {
                            path.AddPolygon(TransformPolygon(
                                hole, cos, sin, placement, placementSheetX,
                                originY, sheetHeight, scale));
                        }
                        e.Graphics.FillPath(fill, path);
                        e.Graphics.DrawPath(outline, path);
                        RectangleF bounds = PolygonBounds(polygon);
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

            private static RectangleF PolygonBounds(System.Drawing.PointF[] polygon)
            {
                float minX = float.MaxValue;
                float minY = float.MaxValue;
                float maxX = float.MinValue;
                float maxY = float.MinValue;
                foreach (System.Drawing.PointF point in polygon)
                {
                    minX = Math.Min(minX, point.X);
                    minY = Math.Min(minY, point.Y);
                    maxX = Math.Max(maxX, point.X);
                    maxY = Math.Max(maxY, point.Y);
                }
                return RectangleF.FromLTRB(minX, minY, maxX, maxY);
            }

            private static string CompactPartLabel(string partId)
            {
                const string prefix = "part-";
                return partId != null && partId.StartsWith(
                    prefix, StringComparison.OrdinalIgnoreCase)
                    ? "#" + partId.Substring(prefix.Length)
                    : partId;
            }
        }
    }
}
