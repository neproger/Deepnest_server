using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
using System.Web.Script.Serialization;

namespace CorelDeepnest.Contracts
{
    public sealed class CorelGateway : MarshalByRefObject, ICorelGateway
    {
        private const int MillimeterUnit = 3;
        private const int SvgFilter = 1345;
        private const int SelectionExport = 2;
        private const int GroupShapeType = 7;
        private const int TextShapeType = 6;
        private readonly dynamic application;
        private static readonly JavaScriptSerializer Json = CreateJsonSerializer();
        private sealed class CapturedPart
        {
            internal object SourceShape;
            internal double OriginX;
            internal double OriginY;
        }

        private readonly Dictionary<string, CapturedPart> capturedParts =
            new Dictionary<string, CapturedPart>(StringComparer.Ordinal);

        public CorelGateway(object application)
        {
            this.application = application;
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
                capturedParts.Add(partId, new CapturedPart
                {
                    SourceShape = source,
                    OriginX = Convert.ToDouble(source.LeftX),
                    // Corel SVG export uses the upper-left corner and a
                    // downward Y axis. Keep the matching Corel anchor so the
                    // server placement can be converted back without drift.
                    OriginY = Convert.ToDouble(source.TopY)
                });
                parts.Add(new Dictionary<string, object>
                {
                    { "id", partId },
                    { "data", svg }
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
                temporaryDocument.ActivePage.Shapes.All().CreateSelection();
                exportFilter = ExportSvg(temporaryDocument, path);
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

        private object ExportSvg(object document, string path)
        {
            Type documentType = FindCorelInteropType(
                "Corel.Interop.VGCore.IVGDocument");
            MethodInfo exportEx = documentType.GetMethod("ExportEx");
            if (exportEx == null)
            {
                throw new MissingMethodException(documentType.FullName, "ExportEx");
            }

            object svgFilter = Enum.ToObject(
                FindCorelInteropType("Corel.Interop.VGCore.cdrFilter"),
                SvgFilter);
            object selectionRange = Enum.ToObject(
                FindCorelInteropType("Corel.Interop.VGCore.cdrExportRange"),
                SelectionExport);
            object exportOptions = application.CreateStructExportOptions();
            object paletteOptions = application.CreateStructPaletteOptions();

            return exportEx.Invoke(document, new object[]
            {
                path,
                svgFilter,
                selectionRange,
                exportOptions,
                paletteOptions
            });
        }

        private static Type FindCorelInteropType(string fullName)
        {
            foreach (Assembly assembly in AppDomain.CurrentDomain.GetAssemblies())
            {
                Type type = assembly.GetType(fullName, false);
                if (type != null)
                {
                    return type;
                }
            }

            throw new TypeLoadException(
                "CorelDRAW interop type is not loaded: " + fullName);
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
            if (operation == "ImportNestingSvg")
            {
                return ImportNestingSvg(payload);
            }
            throw new InvalidOperationException(
                "Unknown Corel gateway operation: " + operation);
        }

        private string ImportNestingSvg(string payload)
        {
            var request = Json.Deserialize<Dictionary<string, object>>(payload);
            string svg = Convert.ToString(request["svg"]);
            if (string.IsNullOrWhiteSpace(svg))
            {
                throw new InvalidOperationException("The nesting SVG result is empty.");
            }

            int placements = Convert.ToInt32(request["placements"]);
            int sheets = Convert.ToInt32(request["sheets"]);
            dynamic document = application.ActiveDocument;
            dynamic page = document.ActivePage;
            string layerName = "Deepnest Result " + DateTime.Now.ToString("HHmmss");
            string directory = Path.Combine(Path.GetTempPath(), "CorelDeepnest");
            Directory.CreateDirectory(directory);
            string path = Path.Combine(
                directory, "result-" + Guid.NewGuid().ToString("N") + ".svg");
            bool commandGroupStarted = false;
            dynamic importFilter = null;
            try
            {
                File.WriteAllText(path, svg);
                document.BeginCommandGroup("Import Deepnest SVG result");
                commandGroupStarted = true;
                dynamic layer = page.CreateLayer(layerName);
                importFilter = ImportSvg(layer, path);
                importFilter.Finish();
                importFilter = null;

                dynamic imported = application.ActiveSelectionRange;
                if (Convert.ToInt32(imported.Count) > 0)
                {
                    imported.Move(
                        Convert.ToDouble(page.LeftX) - Convert.ToDouble(imported.LeftX),
                        Convert.ToDouble(page.BottomY) - Convert.ToDouble(imported.BottomY));
                }

                return Json.Serialize(new Dictionary<string, object>
                {
                    { "applied", placements },
                    { "sheets", sheets },
                    { "layer", layerName }
                });
            }
            finally
            {
                if (importFilter != null)
                {
                    try { importFilter.Finish(); } catch { }
                }
                if (commandGroupStarted)
                {
                    document.EndCommandGroup();
                }
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

        private object ImportSvg(object layer, string path)
        {
            Type layerType = FindCorelInteropType(
                "Corel.Interop.VGCore.IVGLayer");
            MethodInfo importEx = layerType.GetMethod("ImportEx");
            if (importEx == null)
            {
                throw new MissingMethodException(layerType.FullName, "ImportEx");
            }

            object svgFilter = Enum.ToObject(
                FindCorelInteropType("Corel.Interop.VGCore.cdrFilter"),
                SvgFilter);
            object importOptions = application.CreateStructImportOptions();
            return importEx.Invoke(layer, new object[]
            {
                path,
                svgFilter,
                importOptions
            });
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

                    CapturedPart captured;
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
                    double targetY = sheetBottom + sheetHeightDocument -
                        application.ConvertUnits(y, MillimeterUnit, documentUnit);

                    dynamic copy = ((dynamic)captured.SourceShape).Duplicate(0.0, 0.0);
                    copy.MoveToLayer(layer);
                    // SVG coordinates point down while Corel document
                    // coordinates point up, so the rotation changes sign.
                    copy.RotateEx(-rotation, captured.OriginX, captured.OriginY);
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
