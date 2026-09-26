using System;
using System.Collections.Generic;
using System.Net.Http;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Forms;
using Corel.Interop.VGCore;
using CorelApplication = Corel.Interop.VGCore.Application;

namespace CorelDeepnest
{
    public partial class Main
    {
        private static readonly HttpClient Http = new HttpClient
        {
            BaseAddress = new Uri("http://127.0.0.1:8080/"),
            Timeout = TimeSpan.FromSeconds(10)
        };

        private static readonly JavaScriptSerializer Json = new JavaScriptSerializer();

        private void Startup()
        {
        }

        [CgsAddInMacro]
        public void TestDeepnestConnection()
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

        [CgsAddInMacro]
        public void NestSelectedShapes()
        {
            try
            {
                using (NestingForm form = new NestingForm(app))
                {
                    form.ShowDialog();
                }
            }
            catch (Exception error)
            {
                ShowError("Deepnest window failed", error);
            }
        }

        private static string RunJob(CorelApplication application, double sheetWidth,
            double sheetHeight, double spacing, int rotations)
        {
            string jobId = null;

            try
            {
                string requestJson = Json.Serialize(BuildRequest(
                    application, sheetWidth, sheetHeight, spacing, rotations));
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
                DateTime deadline = DateTime.UtcNow.AddSeconds(30);

                while (DateTime.UtcNow < deadline)
                {
                    using (HttpResponseMessage response = Http.GetAsync(
                        "api/v1/jobs/" + jobId + "/result").GetAwaiter().GetResult())
                    {
                        string body = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
                        if (response.IsSuccessStatusCode)
                        {
                            return body;
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

        private static Dictionary<string, object> BuildRequest(CorelApplication application,
            double sheetWidth, double sheetHeight, double spacing, int rotations)
        {
            dynamic selection = application.ActiveSelectionRange;
            int shapeCount = Convert.ToInt32(selection.Count);
            if (shapeCount == 0)
            {
                throw new InvalidOperationException("Select one or more closed curve shapes first.");
            }

            var parts = new List<object>();
            for (int index = 1; index <= shapeCount; index++)
            {
                dynamic shape = selection.Shapes[index];
                parts.Add(new Dictionary<string, object>
                {
                    { "id", "part-" + index },
                    { "quantity", 1 },
                    { "polygontree", PolygonFromShape(shape, index) }
                });
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

        private static Dictionary<string, object> PolygonFromShape(dynamic shape, int partIndex)
        {
            if (Convert.ToInt32(shape.Type) != (int)cdrShapeType.cdrCurveShape)
            {
                throw new InvalidOperationException("Part " + partIndex + " is not a Curve shape.");
            }

            dynamic subPaths = shape.Curve.SubPaths;
            if (Convert.ToInt32(subPaths.Count) != 1)
            {
                throw new InvalidOperationException(
                    "Part " + partIndex + " must have exactly one contour.");
            }

            dynamic subPath = subPaths[1];
            if (!Convert.ToBoolean(subPath.Closed))
            {
                throw new InvalidOperationException("Part " + partIndex + " has an open contour.");
            }

            dynamic segments = subPath.Segments;
            int segmentCount = Convert.ToInt32(segments.Count);
            for (int index = 1; index <= segmentCount; index++)
            {
                dynamic segment = segments[index];
                if (Convert.ToInt32(segment.Type) != (int)cdrSegmentType.cdrLineSegment)
                {
                    throw new InvalidOperationException(
                        "Part " + partIndex + " has curve segments. Only straight lines are supported now.");
                }
            }

            dynamic nodes = subPath.Nodes;
            int nodeCount = Convert.ToInt32(nodes.Count);
            if (nodeCount < 3)
            {
                throw new InvalidOperationException(
                    "Part " + partIndex + " must contain at least three nodes.");
            }

            var points = new object[nodeCount];
            for (int index = 1; index <= nodeCount; index++)
            {
                dynamic node = nodes[index];
                points[index - 1] = Point(
                    Convert.ToDouble(node.PositionX),
                    Convert.ToDouble(node.PositionY));
            }

            return Polygon(points);
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
            private readonly CorelApplication corel;
            private readonly NumericUpDown width = NumberInput(1000);
            private readonly NumericUpDown height = NumberInput(500);
            private readonly NumericUpDown spacing = NumberInput(0);
            private readonly NumericUpDown rotations = NumberInput(4, 0);
            private readonly TextBox result = new TextBox();
            private readonly Button run = new Button();

            public NestingForm(CorelApplication application)
            {
                corel = application;
                Text = "Deepnest Server";
                Width = 760;
                Height = 600;
                StartPosition = FormStartPosition.CenterParent;

                var fields = new TableLayoutPanel
                {
                    Dock = DockStyle.Top,
                    Height = 80,
                    ColumnCount = 8,
                    Padding = new Padding(8)
                };

                AddField(fields, "Sheet width", width, 0);
                AddField(fields, "Sheet height", height, 2);
                AddField(fields, "Spacing", spacing, 4);
                AddField(fields, "Rotations", rotations, 6);

                run.Text = "Run nesting";
                run.Dock = DockStyle.Top;
                run.Height = 36;
                run.Click += RunClick;

                result.Dock = DockStyle.Fill;
                result.Multiline = true;
                result.ScrollBars = ScrollBars.Both;
                result.WordWrap = false;
                result.ReadOnly = true;

                Controls.Add(result);
                Controls.Add(run);
                Controls.Add(fields);
            }

            private void RunClick(object sender, EventArgs e)
            {
                run.Enabled = false;
                result.Text = "Creating nesting job...";
                System.Windows.Forms.Application.DoEvents();

                try
                {
                    result.Text = RunJob(
                        corel,
                        Convert.ToDouble(width.Value),
                        Convert.ToDouble(height.Value),
                        Convert.ToDouble(spacing.Value),
                        Convert.ToInt32(rotations.Value));
                }
                catch (Exception error)
                {
                    result.Text = "Deepnest job failed:" + Environment.NewLine + error.Message;
                }
                finally
                {
                    run.Enabled = true;
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
    }
}
