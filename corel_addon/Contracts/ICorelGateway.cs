namespace CorelDeepnest.Contracts
{
    public interface ICorelGateway
    {
        string CaptureSelectionJson(int curvePrecision);
        string CaptureSelectionSvgJson();
        string Invoke(string operation, string payload);
    }
}
