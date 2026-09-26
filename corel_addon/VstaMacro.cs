using System;
using System.Collections.Generic;
using System.Drawing;
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

        private static readonly JavaScriptSerializer Json = new JavaScriptSerializer();

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
        }

        private sealed class PreviewPlacement
        {
            public string PartId;
            public double X;
            public double Y;
            public double Rotation;
        }

        private sealed class PreviewModel
        {
            public double SheetWidth;
            public double SheetHeight;
            public List<PreviewPart> Parts;
            public List<PreviewPlacement> Placements;
        }

        private sealed class JobRunResult
        {
            public string Json;
            public PreviewModel Preview;
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

        private static JobRunResult RunJob(ICorelGateway gateway, double sheetWidth,
            double sheetHeight, double spacing, int rotations,
            Action<string> jobCreated, Func<bool> stopRequested)
        {
            string jobId = null;
            List<PreviewPart> previewParts;

            try
            {
                string requestJson = Json.Serialize(BuildRequest(
                    gateway, sheetWidth, sheetHeight, spacing, rotations, out previewParts));
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
                DateTime deadline = DateTime.UtcNow.AddSeconds(30);

                while (DateTime.UtcNow < deadline)
                {
                    if (stopRequested())
                    {
                        throw new OperationCanceledException("The nesting job was stopped by the user.");
                    }

                    using (HttpResponseMessage response = Http.GetAsync(
                        "api/v1/jobs/" + jobId + "/result").GetAwaiter().GetResult())
                    {
                        string body = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
                        if (response.IsSuccessStatusCode)
                        {
                            return new JobRunResult
                            {
                                Json = body,
                                Preview = ParsePreview(body, sheetWidth, sheetHeight, previewParts)
                            };
                        }

                        if ((int)response.StatusCode != 409)
                        {
                            ThrowHttpError(response, body);
                        }
                    }

                    System.Windows.Forms.Application.DoEvents();
                    Thread.Sleep(200);
                }

                throw new TimeoutException("Timed out waiting for a nesting result.");
            }
            finally
            {
                if (!string.IsNullOrEmpty(jobId))
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

        private static Dictionary<string, object> BuildRequest(ICorelGateway gateway,
            double sheetWidth, double sheetHeight, double spacing, int rotations,
            out List<PreviewPart> previewParts)
        {
            var selection = Json.Deserialize<Dictionary<string, object>>(
                gateway.CaptureSelectionJson());
            var parts = new List<object>();
            previewParts = new List<PreviewPart>();
            foreach (object partValue in (System.Collections.IEnumerable)selection["parts"])
            {
                var capturedPart = (Dictionary<string, object>)partValue;
                string partId = Convert.ToString(capturedPart["id"]);
                var points = new List<GeometryPoint>();
                var polygonPoints = new List<object>();
                foreach (object pointValue in
                    (System.Collections.IEnumerable)capturedPart["points"])
                {
                    var capturedPoint = (Dictionary<string, object>)pointValue;
                    double x = Convert.ToDouble(capturedPoint["x"]);
                    double y = Convert.ToDouble(capturedPoint["y"]);
                    points.Add(new GeometryPoint { X = x, Y = y });
                    polygonPoints.Add(Point(x, y));
                }

                parts.Add(new Dictionary<string, object>
                {
                    { "id", partId },
                    { "quantity", 1 },
                    { "polygontree", Polygon(polygonPoints.ToArray()) }
                });
                previewParts.Add(new PreviewPart { Id = partId, Points = points });
            }

            object[] sheetPoints =
            {
                Point(0, 0),
                Point(sheetWidth, 0),
                Point(sheetWidth, sheetHeight),
                Point(0, sheetHeight)
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
                { "units", "document" },
                { "sheets", new object[] { sheet } },
                { "parts", parts.ToArray() }
            };

            var config = new Dictionary<string, object>
            {
                { "spacing", spacing },
                { "rotations", rotations },
                { "timeRatio", 0 }
            };

            return new Dictionary<string, object>
            {
                { "input", input },
                { "config", config }
            };
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
            var root = Json.Deserialize<Dictionary<string, object>>(json);
            var placements = new List<PreviewPlacement>();
            object value;

            if (root.TryGetValue("placements", out value))
            {
                foreach (object item in (System.Collections.IEnumerable)value)
                {
                    var placement = (Dictionary<string, object>)item;
                    placements.Add(new PreviewPlacement
                    {
                        PartId = Convert.ToString(placement["partId"]),
                        X = Convert.ToDouble(placement["x"]),
                        Y = Convert.ToDouble(placement["y"]),
                        Rotation = Convert.ToDouble(placement["rotation"])
                    });
                }
            }

            return new PreviewModel
            {
                SheetWidth = sheetWidth,
                SheetHeight = sheetHeight,
                Parts = parts,
                Placements = placements
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
            private readonly TextBox result = new TextBox();
            private readonly Button run = new Button();
            private readonly Button stop = new Button();
            private readonly Label status = new Label();
            private readonly PreviewPanel preview = new PreviewPanel();
            private bool stopRequested;

            public NestingForm(ICorelGateway gateway)
            {
                this.gateway = gateway;
                width.Minimum = 0.01M;
                height.Minimum = 0.01M;
                rotations.Minimum = 1;
                Text = "Deepnest Server — runtime " + BuildInfo.Id;
                Width = 920;
                Height = 680;
                StartPosition = FormStartPosition.CenterParent;

                var fields = new TableLayoutPanel
                {
                    Dock = DockStyle.Top,
                    Height = 80,
                    ColumnCount = 8,
                    Padding = new Padding(8)
                };

                AddField(fields, "Sheet width, mm", width, 0);
                AddField(fields, "Sheet height, mm", height, 2);
                AddField(fields, "Spacing, mm", spacing, 4);
                AddField(fields, "Rotations", rotations, 6);

                run.Text = "Run nesting";
                run.Click += RunClick;

                stop.Text = "Stop";
                stop.Enabled = false;
                stop.Click += delegate { stopRequested = true; status.Text = "Stopping job..."; };

                status.Text = "Select closed line-only curves, then run nesting.";
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
                Controls.Add(fields);
            }

            private void RunClick(object sender, EventArgs e)
            {
                run.Enabled = false;
                stop.Enabled = true;
                stopRequested = false;
                preview.Model = null;
                result.Text = "Creating nesting job...";
                status.Text = "Creating nesting job...";
                System.Windows.Forms.Application.DoEvents();

                try
                {
                    JobRunResult jobResult = RunJob(
                        gateway,
                        Convert.ToDouble(width.Value),
                        Convert.ToDouble(height.Value),
                        Convert.ToDouble(spacing.Value),
                        Convert.ToInt32(rotations.Value),
                        delegate(string jobId) { status.Text = "Job " + jobId + " is running..."; },
                        delegate { return stopRequested; });
                    result.Text = jobResult.Json;
                    preview.Model = jobResult.Preview;
                    status.Text = "Placement complete: " + jobResult.Preview.Placements.Count + " part(s).";
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

            private static void AddField(TableLayoutPanel panel, string label,
                System.Windows.Forms.Control control, int column)
            {
                panel.Controls.Add(new Label
                {
                    Text = label,
                    AutoSize = true,
                    Anchor = AnchorStyles.Left
                }, column, 0);
                panel.Controls.Add(control, column + 1, 0);
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
                float scale = Math.Min(
                    (ClientSize.Width - margin * 2) / (float)model.SheetWidth,
                    (ClientSize.Height - margin * 2) / (float)model.SheetHeight);
                float sheetWidth = (float)model.SheetWidth * scale;
                float sheetHeight = (float)model.SheetHeight * scale;
                float originX = (ClientSize.Width - sheetWidth) / 2;
                float originY = (ClientSize.Height - sheetHeight) / 2;

                using (Brush sheetBrush = new SolidBrush(DrawingColor.WhiteSmoke))
                using (Pen sheetPen = new Pen(DrawingColor.Silver, 2))
                {
                    e.Graphics.FillRectangle(sheetBrush, originX, originY, sheetWidth, sheetHeight);
                    e.Graphics.DrawRectangle(sheetPen, originX, originY, sheetWidth, sheetHeight);
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
                    var polygon = new System.Drawing.PointF[part.Points.Count];

                    for (int pointIndex = 0; pointIndex < part.Points.Count; pointIndex++)
                    {
                        GeometryPoint point = part.Points[pointIndex];
                        double worldX = point.X * cos - point.Y * sin + placement.X;
                        double worldY = point.X * sin + point.Y * cos + placement.Y;
                        polygon[pointIndex] = new System.Drawing.PointF(
                            originX + (float)worldX * scale,
                            originY + sheetHeight - (float)worldY * scale);
                    }

                    DrawingColor color = colors[placementIndex % colors.Length];
                    using (Brush fill = new SolidBrush(color))
                    using (Pen outline = new Pen(DrawingColor.FromArgb(220, color), 1.5f))
                    using (Brush labelBrush = new SolidBrush(DrawingColor.Black))
                    {
                        e.Graphics.FillPolygon(fill, polygon);
                        e.Graphics.DrawPolygon(outline, polygon);
                        e.Graphics.DrawString(placement.PartId + "  " + placement.Rotation + "°",
                            Font, labelBrush, polygon[0]);
                    }
                }
            }
        }
    }
}
