using System;
using System.Collections.Generic;
using System.IO;
using System.Web.Script.Serialization;

namespace CorelDeepnest.Contracts
{
    public sealed class CorelGateway : MarshalByRefObject, ICorelGateway
    {
        private const int MillimeterUnit = 3;
        private const int CopyShapeAppearance = 1 | 2 | 4;
        private const int SvgFilter = 1345;
        private const int CurrentPageExport = 1;
        private const int GroupShapeType = 7;
        private const int TextShapeType = 6;
        private readonly dynamic application;
        private static readonly JavaScriptSerializer Json = CreateJsonSerializer();
        private readonly Dictionary<string, ExtractedPart> capturedParts =
            new Dictionary<string, ExtractedPart>(StringComparer.Ordinal);

        public CorelGateway(object application)
        {
            this.application = application;
        }

        public string CaptureSelectionJson(int curvePrecision)
        {
            List<ExtractedPart> extracted =
                new GeometryExtractor(application, curvePrecision).ExtractSelection();
            var parts = new List<object>();
            capturedParts.Clear();
            foreach (ExtractedPart part in extracted)
            {
                capturedParts.Add(part.Id, part);
                parts.Add(new Dictionary<string, object>
                {
                    { "id", part.Id },
                    { "polygontree", part.PolygonTree },
                    { "geometryMode", part.DuplicateSource
                        ? "group-convex-hull"
                        : "display-curve" }
                });
            }

            return Json.Serialize(new Dictionary<string, object>
            {
                { "curvePrecision", curvePrecision },
                { "parts", parts.ToArray() }
            });
        }

        public string CaptureSelectionSvgJson()
        {
            dynamic document = application.ActiveDocument;
            dynamic selection = application.ActiveSelectionRange;
            int shapeCount = Convert.ToInt32(selection.Count);
            if (shapeCount == 0)
            {
                throw new InvalidOperationException(
                    "Select one or more vector shapes first.");
            }

            var sourceShapes = new List<object>();
            for (int index = 1; index <= shapeCount; index++)
            {
                sourceShapes.Add(selection.Shapes[index]);
            }

            capturedParts.Clear();
            var parts = new List<object>();
            for (int index = 0; index < sourceShapes.Count; index++)
            {
                dynamic source = sourceShapes[index];
                string partId = "part-" + (index + 1);
                string svg = ExportShapeSvg(source, document, partId);
                capturedParts.Add(partId, new ExtractedPart
                {
                    Id = partId,
                    SourceShape = source,
                    DuplicateSource = true,
                    OriginX = Convert.ToDouble(source.LeftX),
                    OriginY = Convert.ToDouble(source.BottomY)
                });
                parts.Add(new Dictionary<string, object>
                {
                    { "id", partId },
                    { "data", svg },
                    { "geometryMode", "corel-svg" }
                });
            }

            document.Activate();
            document.ClearSelection();
            foreach (dynamic source in sourceShapes)
            {
                source.AddToSelection();
            }

            return Json.Serialize(new Dictionary<string, object>
            {
                { "parts", parts.ToArray() }
            });
        }

        private string ExportShapeSvg(dynamic source, dynamic sourceDocument,
            string partId)
        {
            string directory = Path.Combine(Path.GetTempPath(), "CorelDeepnest");
            Directory.CreateDirectory(directory);
            string path = Path.Combine(
                directory, partId + "-" + Guid.NewGuid().ToString("N") + ".svg");
            dynamic temporaryDocument = null;
            dynamic exportFilter = null;
            try
            {
                temporaryDocument = source.CreateDocumentFrom(true);
                temporaryDocument.Activate();
                ConvertTextToCurves(temporaryDocument.ActivePage.Shapes);
                exportFilter = temporaryDocument.ExportEx(
                    path, SvgFilter, CurrentPageExport);
                exportFilter.Finish();
                exportFilter = null;

                if (!File.Exists(path))
                {
                    throw new InvalidOperationException(
                        "CorelDRAW did not create SVG for " + partId + ".");
                }
                string svg = File.ReadAllText(path);
                if (string.IsNullOrWhiteSpace(svg) ||
                    svg.IndexOf("<svg", StringComparison.OrdinalIgnoreCase) < 0)
                {
                    throw new InvalidOperationException(
                        "CorelDRAW created an invalid SVG for " + partId + ".");
                }
                SaveDiagnosticSvg(partId, svg);
                return svg;
            }
            finally
            {
                if (exportFilter != null)
                {
                    try { exportFilter.Finish(); } catch { }
                }
                if (temporaryDocument != null)
                {
                    try { temporaryDocument.Close(); } catch { }
                }
                try { sourceDocument.Activate(); } catch { }
                try
                {
                    if (File.Exists(path))
                    {
                        File.Delete(path);
                    }
                }
                catch
                {
                }
            }
        }

        private static void SaveDiagnosticSvg(string partId, string svg)
        {
            string directory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "CorelDeepnest", "SvgExport");
            Directory.CreateDirectory(directory);
            File.WriteAllText(Path.Combine(directory, partId + ".svg"), svg);
        }

        private static JavaScriptSerializer CreateJsonSerializer()
        {
            var serializer = new JavaScriptSerializer();
            serializer.MaxJsonLength = 12 * 1024 * 1024;
            return serializer;
        }

        private static void ConvertTextToCurves(dynamic shapes)
        {
            int count = Convert.ToInt32(shapes.Count);
            for (int index = 1; index <= count; index++)
            {
                dynamic shape = shapes[index];
                int type = Convert.ToInt32(shape.Type);
                if (type == GroupShapeType)
                {
                    ConvertTextToCurves(shape.Shapes);
                }
                else if (type == TextShapeType)
                {
                    shape.ConvertToCurves();
                }
            }
        }

        public string Invoke(string operation, string payload)
        {
            if (operation == "ApplyPlacements")
            {
                return ApplyPlacements(payload);
            }
            throw new InvalidOperationException(
                "Unknown Corel gateway operation: " + operation);
        }

        private string ApplyPlacements(string payload)
        {
            if (capturedParts.Count == 0)
            {
                throw new InvalidOperationException(
                    "Run nesting before applying placements.");
            }

            var request = Json.Deserialize<Dictionary<string, object>>(payload);
            double sheetWidth = RequiredDouble(request, "sheetWidth");
            double sheetHeight = RequiredDouble(request, "sheetHeight");
            double sheetGap = RequiredDouble(request, "sheetGap");
            object placementsValue;
            if (!request.TryGetValue("placements", out placementsValue) ||
                placementsValue == null)
            {
                throw new InvalidOperationException("The placement result is empty.");
            }

            dynamic document = application.ActiveDocument;
            dynamic page = document.ActivePage;
            dynamic documentUnit = document.Unit;
            double sheetWidthDocument = application.ConvertUnits(
                sheetWidth, MillimeterUnit, documentUnit);
            double sheetHeightDocument = application.ConvertUnits(
                sheetHeight, MillimeterUnit, documentUnit);
            double sheetGapDocument = application.ConvertUnits(
                sheetGap, MillimeterUnit, documentUnit);
            double sheetLeft = Convert.ToDouble(page.LeftX);
            double sheetBottom = Convert.ToDouble(page.BottomY);
            string layerName = "Deepnest Result " + DateTime.Now.ToString("HHmmss");
            int applied = 0;
            bool commandGroupStarted = false;

            try
            {
                document.BeginCommandGroup("Apply Deepnest placements");
                commandGroupStarted = true;
                dynamic layer = page.CreateLayer(layerName);
                var copies = new List<object>();
                var usedSheetInstances = new SortedSet<int>();

                foreach (object placementValue in
                    (System.Collections.IEnumerable)placementsValue)
                {
                    var placement = (Dictionary<string, object>)placementValue;
                    string partId = Convert.ToString(placement["partId"]);
                    int sheetInstanceId = placement.ContainsKey("sheetInstanceId")
                        ? Convert.ToInt32(placement["sheetInstanceId"])
                        : 0;
                    if (sheetInstanceId < 0)
                    {
                        throw new InvalidOperationException(
                            "Placement has an invalid sheetInstanceId.");
                    }

                    ExtractedPart captured;
                    if (!capturedParts.TryGetValue(partId, out captured))
                    {
                        throw new InvalidOperationException(
                            "No captured Corel component matches " + partId + ".");
                    }

                    double x = RequiredDouble(placement, "x");
                    double y = RequiredDouble(placement, "y");
                    double rotation = RequiredDouble(placement, "rotation");
                    double instanceLeft = sheetLeft + sheetInstanceId *
                        (sheetWidthDocument + sheetGapDocument);
                    double targetX = instanceLeft + application.ConvertUnits(
                        x, MillimeterUnit, documentUnit);
                    double targetY = sheetBottom + application.ConvertUnits(
                        y, MillimeterUnit, documentUnit);

                    dynamic copy;
                    if (captured.DuplicateSource)
                    {
                        copy = ((dynamic)captured.SourceShape).Duplicate(0.0, 0.0);
                        copy.MoveToLayer(layer);
                    }
                    else
                    {
                        copy = layer.CreateCurve(((dynamic)captured.Curve).GetCopy());
                        copy.CopyPropertiesFrom(
                            (dynamic)captured.SourceShape, CopyShapeAppearance);
                    }
                    copy.RotateEx(rotation, captured.OriginX, captured.OriginY);
                    copy.Move(
                        targetX - captured.OriginX,
                        targetY - captured.OriginY);
                    copy.Name = "Deepnest " + partId;
                    copies.Add(copy);
                    usedSheetInstances.Add(sheetInstanceId);
                    applied++;
                }

                foreach (int sheetInstanceId in usedSheetInstances)
                {
                    double instanceLeft = sheetLeft + sheetInstanceId *
                        (sheetWidthDocument + sheetGapDocument);
                    dynamic sheet = layer.CreateRectangle(
                        instanceLeft,
                        sheetBottom + sheetHeightDocument,
                        instanceLeft + sheetWidthDocument,
                        sheetBottom,
                        0, 0, 0, 0);
                    sheet.Name = "Deepnest Sheet " + (sheetInstanceId + 1);
                    sheet.OrderToBack();
                }

                document.ClearSelection();
                foreach (dynamic copy in copies)
                {
                    copy.AddToSelection();
                }
            }
            finally
            {
                if (commandGroupStarted)
                {
                    document.EndCommandGroup();
                }
            }

            return Json.Serialize(new Dictionary<string, object>
            {
                { "applied", applied },
                { "sheets", CountSheetInstances(placementsValue) },
                { "layer", layerName }
            });
        }

        private static int CountSheetInstances(object placementsValue)
        {
            var instances = new HashSet<int>();
            foreach (object placementValue in
                (System.Collections.IEnumerable)placementsValue)
            {
                var placement = (Dictionary<string, object>)placementValue;
                instances.Add(placement.ContainsKey("sheetInstanceId")
                    ? Convert.ToInt32(placement["sheetInstanceId"])
                    : 0);
            }
            return instances.Count;
        }

        private static double RequiredDouble(
            Dictionary<string, object> values, string name)
        {
            object value;
            if (!values.TryGetValue(name, out value) || value == null)
            {
                throw new InvalidOperationException(
                    "Placement data has no " + name + ".");
            }
            return Convert.ToDouble(value);
        }

        public override object InitializeLifetimeService()
        {
            return null;
        }
    }
}
