using System;
using System.Collections.Generic;
using System.Web.Script.Serialization;

namespace CorelDeepnest.Contracts
{
    public sealed class CorelGateway : MarshalByRefObject, ICorelGateway
    {
        private const int CurveShapeType = 3;
        private const int LineSegmentType = 0;
        private const int MillimeterUnit = 3;
        private readonly dynamic application;
        private static readonly JavaScriptSerializer Json = new JavaScriptSerializer();

        public CorelGateway(object application)
        {
            this.application = application;
        }

        public string CaptureSelectionJson()
        {
            dynamic selection = application.ActiveSelectionRange;
            int shapeCount = Convert.ToInt32(selection.Count);
            if (shapeCount == 0)
            {
                throw new InvalidOperationException(
                    "Select one or more closed curve shapes first.");
            }

            var parts = new List<object>();
            for (int index = 1; index <= shapeCount; index++)
            {
                dynamic shape = selection.Shapes[index];
                parts.Add(new Dictionary<string, object>
                {
                    { "id", "part-" + index },
                    { "points", PointsFromShape(shape, index) }
                });
            }

            return Json.Serialize(new Dictionary<string, object>
            {
                { "parts", parts.ToArray() }
            });
        }

        private object[] PointsFromShape(dynamic shape, int partIndex)
        {
            if (Convert.ToInt32(shape.Type) != CurveShapeType)
            {
                throw new InvalidOperationException(
                    "Part " + partIndex + " is not a Curve shape.");
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
                throw new InvalidOperationException(
                    "Part " + partIndex + " has an open contour.");
            }

            dynamic segments = subPath.Segments;
            int segmentCount = Convert.ToInt32(segments.Count);
            for (int index = 1; index <= segmentCount; index++)
            {
                if (Convert.ToInt32(segments[index].Type) != LineSegmentType)
                {
                    throw new InvalidOperationException(
                        "Part " + partIndex +
                        " has curve segments. Only straight lines are supported now.");
                }
            }

            dynamic nodes = subPath.Nodes;
            int nodeCount = Convert.ToInt32(nodes.Count);
            if (nodeCount < 3)
            {
                throw new InvalidOperationException(
                    "Part " + partIndex + " must contain at least three nodes.");
            }

            object documentUnit = application.ActiveDocument.Unit;
            var source = new List<double[]>();
            double minX = double.MaxValue;
            double minY = double.MaxValue;
            for (int index = 1; index <= nodeCount; index++)
            {
                dynamic node = nodes[index];
                double x = application.ConvertUnits(
                    Convert.ToDouble(node.PositionX), documentUnit, MillimeterUnit);
                double y = application.ConvertUnits(
                    Convert.ToDouble(node.PositionY), documentUnit, MillimeterUnit);
                source.Add(new[] { x, y });
                minX = Math.Min(minX, x);
                minY = Math.Min(minY, y);
            }

            var points = new object[source.Count];
            for (int index = 0; index < source.Count; index++)
            {
                points[index] = new Dictionary<string, object>
                {
                    { "x", source[index][0] - minX },
                    { "y", source[index][1] - minY }
                };
            }
            return points;
        }

        public override object InitializeLifetimeService()
        {
            return null;
        }
    }
}
