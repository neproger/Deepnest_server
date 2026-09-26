using System;
using System.Collections.Generic;

namespace CorelDeepnest.Contracts
{
    internal sealed class ExtractedPart
    {
        internal string Id;
        internal object SourceShape;
        internal object Curve;
        internal double OriginX;
        internal double OriginY;
        internal Dictionary<string, object> PolygonTree;
        internal bool DuplicateSource;
    }

    internal sealed class GeometryExtractor
    {
        private const int MillimeterUnit = 3;
        private const int GroupShapeType = 7;
        private const int MaximumPointsPerContour = 10000;
        private const double Epsilon = 0.000001;
        private readonly dynamic application;
        private readonly int curvePrecision;

        private sealed class Point
        {
            internal double X;
            internal double Y;
        }

        private sealed class Contour
        {
            internal int Index;
            internal object SourceSubPath;
            internal List<Point> Points;
            internal double Area;
            internal Contour Parent;
            internal readonly List<Contour> Children = new List<Contour>();

            internal int Depth
            {
                get
                {
                    int value = 0;
                    for (Contour current = Parent; current != null; current = current.Parent)
                    {
                        value++;
                    }
                    return value;
                }
            }
        }

        internal GeometryExtractor(object application, int curvePrecision)
        {
            if (curvePrecision < 1 || curvePrecision > 100)
            {
                throw new ArgumentOutOfRangeException(
                    "curvePrecision", "Corel curve precision must be from 1 to 100.");
            }
            this.application = application;
            this.curvePrecision = curvePrecision;
        }

        internal List<ExtractedPart> ExtractSelection()
        {
            dynamic selection = application.ActiveSelectionRange;
            int shapeCount = Convert.ToInt32(selection.Count);
            if (shapeCount == 0)
            {
                throw new InvalidOperationException(
                    "Select one or more closed vector shapes first.");
            }

            var result = new List<ExtractedPart>();
            for (int shapeIndex = 1; shapeIndex <= shapeCount; shapeIndex++)
            {
                dynamic shape = selection.Shapes[shapeIndex];
                if (Convert.ToInt32(shape.Type) == GroupShapeType)
                {
                    result.Add(CreateGroupPart(
                        "part-" + (result.Count + 1), shape, shapeIndex));
                    continue;
                }
                List<Contour> contours = ReadContours(shape, shapeIndex);
                BuildContainment(contours, shapeIndex);
                foreach (Contour outer in contours)
                {
                    if ((outer.Depth & 1) == 0)
                    {
                        result.Add(CreatePart(
                            "part-" + (result.Count + 1), shape, outer));
                    }
                }
            }

            if (result.Count == 0)
            {
                throw new InvalidOperationException(
                    "The selection contains no closed solid contours.");
            }
            return result;
        }

        private ExtractedPart CreateGroupPart(
            string id, dynamic group, int selectionIndex)
        {
            var contours = new List<Contour>();
            CollectGroupContours(group, selectionIndex, contours);
            var allPoints = new List<Point>();
            foreach (Contour contour in contours)
            {
                allPoints.AddRange(contour.Points);
            }
            List<Point> hull = ConvexHull(allPoints);
            if (hull.Count < 3)
            {
                throw new InvalidOperationException(
                    "Group " + selectionIndex +
                    " does not contain enough closed vector geometry.");
            }

            double minX = double.MaxValue;
            double minY = double.MaxValue;
            IncludeMinimum(hull, ref minX, ref minY);
            dynamic documentUnit = application.ActiveDocument.Unit;
            return new ExtractedPart
            {
                Id = id,
                SourceShape = group,
                Curve = null,
                DuplicateSource = true,
                OriginX = application.ConvertUnits(minX, MillimeterUnit, documentUnit),
                OriginY = application.ConvertUnits(minY, MillimeterUnit, documentUnit),
                PolygonTree = new Dictionary<string, object>
                {
                    { "points", PointDtos(hull, minX, minY) },
                    { "children", new object[0] }
                }
            };
        }

        private void CollectGroupContours(
            dynamic group, int selectionIndex, List<Contour> contours)
        {
            dynamic shapes = group.Shapes;
            int count = Convert.ToInt32(shapes.Count);
            if (count == 0)
            {
                throw new InvalidOperationException(
                    "Group " + selectionIndex + " is empty.");
            }
            for (int index = 1; index <= count; index++)
            {
                dynamic child = shapes[index];
                if (Convert.ToInt32(child.Type) == GroupShapeType)
                {
                    CollectGroupContours(child, selectionIndex, contours);
                }
                else
                {
                    contours.AddRange(ReadContours(child, selectionIndex));
                }
            }
        }

        private List<Contour> ReadContours(dynamic shape, int shapeIndex)
        {
            dynamic displayCurve;
            try
            {
                displayCurve = shape.DisplayCurve;
            }
            catch (Exception error)
            {
                throw new InvalidOperationException(
                    "Shape " + shapeIndex +
                    " cannot be represented as a Corel curve. " + error.Message);
            }
            if (displayCurve == null)
            {
                throw new InvalidOperationException(
                    "Shape " + shapeIndex +
                    " has no display curve. Convert unsupported effects or objects to curves first.");
            }

            dynamic curve = displayCurve.GetCopy();
            dynamic subPaths = curve.SubPaths;
            int count = Convert.ToInt32(subPaths.Count);
            if (count == 0)
            {
                throw new InvalidOperationException(
                    "Shape " + shapeIndex + " contains no contours.");
            }

            var contours = new List<Contour>();
            for (int index = 1; index <= count; index++)
            {
                dynamic source = subPaths[index];
                if (!Convert.ToBoolean(source.Closed))
                {
                    throw new InvalidOperationException(
                        "Shape " + shapeIndex + ", contour " + index +
                        " is open. Nesting requires closed contours.");
                }

                dynamic polyline = source.GetPolyline(curvePrecision);
                dynamic polygonSubPaths = polyline.SubPaths;
                if (Convert.ToInt32(polygonSubPaths.Count) != 1)
                {
                    throw new InvalidOperationException(
                        "Shape " + shapeIndex + ", contour " + index +
                        " could not be converted to one polygon.");
                }

                List<Point> points = ReadPoints(
                    polygonSubPaths[1], shapeIndex, index);
                ValidatePolygon(points, shapeIndex, index);
                contours.Add(new Contour
                {
                    Index = index,
                    SourceSubPath = source,
                    Points = points,
                    Area = Math.Abs(SignedArea(points))
                });
            }
            return contours;
        }

        private List<Point> ReadPoints(
            dynamic subPath, int shapeIndex, int contourIndex)
        {
            dynamic nodes = subPath.Nodes;
            int count = Convert.ToInt32(nodes.Count);
            if (count > MaximumPointsPerContour)
            {
                throw new InvalidOperationException(
                    "Shape " + shapeIndex + ", contour " + contourIndex +
                    " has too many polygon points (" + count + ").");
            }

            dynamic documentUnit = application.ActiveDocument.Unit;
            var points = new List<Point>();
            for (int index = 1; index <= count; index++)
            {
                dynamic node = nodes[index];
                var point = new Point
                {
                    X = application.ConvertUnits(
                        Convert.ToDouble(node.PositionX), documentUnit, MillimeterUnit),
                    Y = application.ConvertUnits(
                        Convert.ToDouble(node.PositionY), documentUnit, MillimeterUnit)
                };
                if (points.Count == 0 || !Same(points[points.Count - 1], point))
                {
                    points.Add(point);
                }
            }
            if (points.Count > 1 && Same(points[0], points[points.Count - 1]))
            {
                points.RemoveAt(points.Count - 1);
            }
            RemoveCollinear(points);
            if (points.Count < 3)
            {
                throw new InvalidOperationException(
                    "Shape " + shapeIndex + ", contour " + contourIndex +
                    " has fewer than three distinct polygon points.");
            }
            return points;
        }

        private ExtractedPart CreatePart(
            string id, dynamic sourceShape, Contour outer)
        {
            double minX = double.MaxValue;
            double minY = double.MaxValue;
            IncludeMinimum(outer.Points, ref minX, ref minY);
            foreach (Contour hole in outer.Children)
            {
                IncludeMinimum(hole.Points, ref minX, ref minY);
            }

            dynamic document = application.ActiveDocument;
            dynamic component = application.CreateCurve(document);
            component.AppendCurve(((dynamic)outer.SourceSubPath).GetCopy());
            foreach (Contour hole in outer.Children)
            {
                component.AppendCurve(((dynamic)hole.SourceSubPath).GetCopy());
            }

            dynamic documentUnit = document.Unit;
            return new ExtractedPart
            {
                Id = id,
                SourceShape = sourceShape,
                Curve = component,
                OriginX = application.ConvertUnits(minX, MillimeterUnit, documentUnit),
                OriginY = application.ConvertUnits(minY, MillimeterUnit, documentUnit),
                PolygonTree = PolygonDto(outer, minX, minY)
            };
        }

        private static List<Point> ConvexHull(List<Point> source)
        {
            var points = new List<Point>(source);
            points.Sort(delegate(Point left, Point right)
            {
                int x = left.X.CompareTo(right.X);
                return x != 0 ? x : left.Y.CompareTo(right.Y);
            });

            var unique = new List<Point>();
            foreach (Point point in points)
            {
                if (unique.Count == 0 || !Same(unique[unique.Count - 1], point))
                {
                    unique.Add(point);
                }
            }
            if (unique.Count <= 2)
            {
                return unique;
            }

            var lower = new List<Point>();
            foreach (Point point in unique)
            {
                while (lower.Count >= 2 && Cross(
                    lower[lower.Count - 2], lower[lower.Count - 1], point) <= Epsilon)
                {
                    lower.RemoveAt(lower.Count - 1);
                }
                lower.Add(point);
            }

            var upper = new List<Point>();
            for (int index = unique.Count - 1; index >= 0; index--)
            {
                Point point = unique[index];
                while (upper.Count >= 2 && Cross(
                    upper[upper.Count - 2], upper[upper.Count - 1], point) <= Epsilon)
                {
                    upper.RemoveAt(upper.Count - 1);
                }
                upper.Add(point);
            }
            lower.RemoveAt(lower.Count - 1);
            upper.RemoveAt(upper.Count - 1);
            lower.AddRange(upper);
            return lower;
        }

        private static void BuildContainment(
            List<Contour> contours, int shapeIndex)
        {
            for (int left = 0; left < contours.Count; left++)
            {
                for (int right = left + 1; right < contours.Count; right++)
                {
                    if (PolygonsIntersect(contours[left].Points, contours[right].Points))
                    {
                        throw new InvalidOperationException(
                            "Shape " + shapeIndex + ": contours " +
                            contours[left].Index + " and " + contours[right].Index +
                            " touch or cross. Separate or repair them before nesting.");
                    }
                }
            }

            foreach (Contour child in contours)
            {
                Contour parent = null;
                foreach (Contour candidate in contours)
                {
                    if (candidate == child || candidate.Area <= child.Area)
                    {
                        continue;
                    }
                    int position = PointInPolygon(child.Points[0], candidate.Points);
                    if (position < 0)
                    {
                        throw new InvalidOperationException(
                            "Shape " + shapeIndex + ": contour " + child.Index +
                            " touches contour " + candidate.Index + ".");
                    }
                    if (position > 0 && (parent == null || candidate.Area < parent.Area))
                    {
                        parent = candidate;
                    }
                }
                child.Parent = parent;
                if (parent != null)
                {
                    parent.Children.Add(child);
                }
            }
        }

        private static Dictionary<string, object> PolygonDto(
            Contour contour, double originX, double originY)
        {
            var holes = new List<object>();
            foreach (Contour child in contour.Children)
            {
                holes.Add(new Dictionary<string, object>
                {
                    { "points", PointDtos(child.Points, originX, originY) },
                    { "children", new object[0] }
                });
            }
            return new Dictionary<string, object>
            {
                { "points", PointDtos(contour.Points, originX, originY) },
                { "children", holes.ToArray() }
            };
        }

        private static object[] PointDtos(
            List<Point> points, double originX, double originY)
        {
            var result = new object[points.Count];
            for (int index = 0; index < points.Count; index++)
            {
                result[index] = new Dictionary<string, object>
                {
                    { "x", points[index].X - originX },
                    { "y", points[index].Y - originY }
                };
            }
            return result;
        }

        private static void IncludeMinimum(
            List<Point> points, ref double minX, ref double minY)
        {
            foreach (Point point in points)
            {
                minX = Math.Min(minX, point.X);
                minY = Math.Min(minY, point.Y);
            }
        }

        private static void RemoveCollinear(List<Point> points)
        {
            bool removed;
            do
            {
                removed = false;
                for (int index = 0; index < points.Count && points.Count >= 3; index++)
                {
                    Point before = points[(index + points.Count - 1) % points.Count];
                    Point current = points[index];
                    Point after = points[(index + 1) % points.Count];
                    double scale = Math.Max(1.0,
                        Distance(before, current) + Distance(current, after));
                    if (Math.Abs(Cross(before, current, after)) <= Epsilon * scale)
                    {
                        points.RemoveAt(index);
                        removed = true;
                        break;
                    }
                }
            }
            while (removed);
        }

        private static void ValidatePolygon(
            List<Point> points, int shapeIndex, int contourIndex)
        {
            if (Math.Abs(SignedArea(points)) <= Epsilon)
            {
                throw new InvalidOperationException(
                    "Shape " + shapeIndex + ", contour " + contourIndex +
                    " has zero or negligible area.");
            }
            for (int first = 0; first < points.Count; first++)
            {
                int firstNext = (first + 1) % points.Count;
                for (int second = first + 1; second < points.Count; second++)
                {
                    int secondNext = (second + 1) % points.Count;
                    if (firstNext == second || secondNext == first)
                    {
                        continue;
                    }
                    if (SegmentsIntersect(
                        points[first], points[firstNext],
                        points[second], points[secondNext]))
                    {
                        throw new InvalidOperationException(
                            "Shape " + shapeIndex + ", contour " + contourIndex +
                            " intersects itself.");
                    }
                }
            }
        }

        private static bool PolygonsIntersect(List<Point> left, List<Point> right)
        {
            for (int a = 0; a < left.Count; a++)
            {
                for (int b = 0; b < right.Count; b++)
                {
                    if (SegmentsIntersect(
                        left[a], left[(a + 1) % left.Count],
                        right[b], right[(b + 1) % right.Count]))
                    {
                        return true;
                    }
                }
            }
            return false;
        }

        private static bool SegmentsIntersect(Point a, Point b, Point c, Point d)
        {
            double abC = Cross(a, b, c);
            double abD = Cross(a, b, d);
            double cdA = Cross(c, d, a);
            double cdB = Cross(c, d, b);
            if (((abC > Epsilon && abD < -Epsilon) ||
                 (abC < -Epsilon && abD > Epsilon)) &&
                ((cdA > Epsilon && cdB < -Epsilon) ||
                 (cdA < -Epsilon && cdB > Epsilon)))
            {
                return true;
            }
            return (Math.Abs(abC) <= Epsilon && OnSegment(a, b, c)) ||
                   (Math.Abs(abD) <= Epsilon && OnSegment(a, b, d)) ||
                   (Math.Abs(cdA) <= Epsilon && OnSegment(c, d, a)) ||
                   (Math.Abs(cdB) <= Epsilon && OnSegment(c, d, b));
        }

        private static int PointInPolygon(Point point, List<Point> polygon)
        {
            bool inside = false;
            for (int current = 0, previous = polygon.Count - 1;
                current < polygon.Count; previous = current++)
            {
                Point a = polygon[previous];
                Point b = polygon[current];
                if (Math.Abs(Cross(a, b, point)) <= Epsilon &&
                    OnSegment(a, b, point))
                {
                    return -1;
                }
                if ((a.Y > point.Y) != (b.Y > point.Y) &&
                    point.X < (b.X - a.X) * (point.Y - a.Y) /
                    (b.Y - a.Y) + a.X)
                {
                    inside = !inside;
                }
            }
            return inside ? 1 : 0;
        }

        private static bool OnSegment(Point a, Point b, Point point)
        {
            return point.X >= Math.Min(a.X, b.X) - Epsilon &&
                   point.X <= Math.Max(a.X, b.X) + Epsilon &&
                   point.Y >= Math.Min(a.Y, b.Y) - Epsilon &&
                   point.Y <= Math.Max(a.Y, b.Y) + Epsilon;
        }

        private static double SignedArea(List<Point> points)
        {
            double value = 0;
            for (int index = 0; index < points.Count; index++)
            {
                Point current = points[index];
                Point next = points[(index + 1) % points.Count];
                value += current.X * next.Y - next.X * current.Y;
            }
            return value / 2.0;
        }

        private static double Cross(Point a, Point b, Point c)
        {
            return (b.X - a.X) * (c.Y - a.Y) -
                   (b.Y - a.Y) * (c.X - a.X);
        }

        private static double Distance(Point a, Point b)
        {
            double x = b.X - a.X;
            double y = b.Y - a.Y;
            return Math.Sqrt(x * x + y * y);
        }

        private static bool Same(Point left, Point right)
        {
            return Math.Abs(left.X - right.X) <= Epsilon &&
                   Math.Abs(left.Y - right.Y) <= Epsilon;
        }
    }
}
