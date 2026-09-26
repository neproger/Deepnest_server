using System;
using System.Collections.Generic;
using System.Web.Script.Serialization;

namespace CorelDeepnest.Contracts
{
    public sealed class CorelGateway : MarshalByRefObject, ICorelGateway
    {
        private const int MillimeterUnit = 3;
        private const int CopyShapeAppearance = 1 | 2 | 4;
        private readonly dynamic application;
        private static readonly JavaScriptSerializer Json = new JavaScriptSerializer();
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
