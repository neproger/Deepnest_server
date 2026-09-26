namespace CorelDeepnest.Contracts
{
    public interface ICorelGateway
    {
        string CaptureSelectionSvgJson();
        string Invoke(string operation, string payload);
    }
}
