using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Threading;
using System.Threading.Tasks;
using BepInEx.Configuration;
using UnityEngine;

namespace DSPRemoteSaves
{
    public partial class DSPRemoteSaves
    {
        /// <summary>
        /// 游戏内云存档同步面板（F8 / Configuration Manager 入口打开）。
        /// 只负责 UI 与手动传输调度，网络细节复用插件主体。
        /// </summary>
        internal sealed class SavePanel
        {
            #region 常量与配色
            private const float PanelW = 1160f;
            private const float PanelH = 700f;
            private const float HeaderH = 44f;
            private const float ToolbarH = 40f;
            private const float TableHeadH = 26f;
            private const float FooterH = 44f;
            private const float RowH = 58f;
            private const float ProgressH = 30f;
            private const float ErrorH = 24f;
            private const float ListY = HeaderH + ToolbarH + TableHeadH;
            private const float ListH = PanelH - ListY - FooterH;
            private const float Radius = 10f;

            private static readonly Color PanelBg = C(12, 33, 47, 0.96f);
            private static readonly Color HeaderBg = C(27, 62, 84, 0.98f);
            private static readonly Color ToolbarBg = C(17, 44, 59, 0.98f);
            private static readonly Color TableHeadBg = C(15, 38, 52, 0.98f);
            private static readonly Color FooterBg = C(15, 38, 52, 0.98f);
            private static readonly Color RowBgA = C(22, 49, 64, 0.72f);
            private static readonly Color RowBgB = C(18, 42, 56, 0.62f);
            private static readonly Color RowHover = C(34, 72, 93, 0.78f);
            private static readonly Color Separator = C(35, 68, 87, 0.55f);
            private static readonly Color Border = C(48, 84, 105, 0.75f);

            private static readonly Color TextPrimary = C(232, 241, 246, 1f);
            private static readonly Color TextSecondary = C(154, 178, 192, 1f);
            private static readonly Color TextDim = C(103, 128, 143, 1f);
            private static readonly Color TextFaint = C(82, 105, 119, 1f);

            private static readonly Color AccentCyan = C(121, 214, 232, 1f);
            private static readonly Color AccentAmber = C(233, 193, 124, 1f);
            private static readonly Color AccentGreen = C(126, 214, 168, 1f);
            private static readonly Color AccentRed = C(226, 132, 116, 1f);

            private static readonly Color BtnUploadBg = C(18, 62, 80, 0.95f);
            private static readonly Color BtnUploadBorder = C(79, 195, 221, 0.9f);
            private static readonly Color BtnDownloadBg = C(69, 53, 31, 0.95f);
            private static readonly Color BtnDownloadBorder = C(217, 174, 106, 0.9f);
            private static readonly Color ChipBg = C(18, 54, 74, 0.9f);
            private static readonly Color ChipBorder = C(46, 106, 133, 0.9f);
            private static readonly Color ProgressTrack = C(32, 59, 76, 0.9f);
            private static readonly Color ProgressUp = C(97, 203, 224, 1f);
            private static readonly Color ProgressDown = C(227, 182, 120, 1f);
            #endregion

            #region 状态
            private enum SortMode { Recent, Name, Size }
            private enum DirectionFilter { All, Upload, Download, Synced }
            private enum DropdownKind { None, Sort, Filter }
            private enum TransferKind { None, Upload, Download }
            private enum RowStatus { Idle, Queued, Running, Failed }
            private enum RowDirection { None, Upload, Download, Synced }
            private enum ConnState { Idle, Connecting, Connected, Failed }

            private readonly DSPRemoteSaves _plugin;
            private readonly SemaphoreSlim _uploadSlots;
            private readonly SemaphoreSlim _downloadSlots;

            private readonly List<SaveRow> _rows = new List<SaveRow>();
            private readonly List<SaveRow> _view = new List<SaveRow>();

            private bool _visible;
            private bool _loading;
            private bool _refreshing;
            private ConnState _connection = ConnState.Idle;
            private string _listError;
            private string _search = string.Empty;
            private SortMode _sort = SortMode.Recent;
            private DirectionFilter _filter = DirectionFilter.All;
            private DropdownKind _openDropdown = DropdownKind.None;
            private float _scroll;
            private float _contentHeight;
            private DateTime _nextAutoRefreshUtc = DateTime.MaxValue;
            private DateTime _scheduledRefreshUtc = DateTime.MaxValue;
            private string _quotaText = "—";
            private long _quotaBytes;
            private long _usedBytes;
            private long _reservedBytes;
            private bool _hasQuota;
            private bool _cursorCaptured;
            private bool _cursorVisible;
            private CursorLockMode _cursorLock;

            // 绘制缓存
            private Font _font;
            private float _scale = 1f;
            private float _builtScale = -1f;
            private float _px;
            private float _py;
            private readonly Dictionary<long, Texture2D> _circleCache = new Dictionary<long, Texture2D>();
            private Texture2D _white;
            private Texture2D _logo;
            private Texture2D _magnifier;
            private Texture2D _searchBgTex;
            private GUIStyle _styleTitle;
            private GUIStyle _styleSubtitle;
            private GUIStyle _styleRowName;
            private GUIStyle _styleMetaRight;
            private GUIStyle _styleRight;
            private GUIStyle _styleSmallRight;
            private GUIStyle _styleSmallPrimary;
            private GUIStyle _styleDimSmall;
            private GUIStyle _styleTableHead;
            private GUIStyle _styleTableHeadRight;
            private GUIStyle _styleTableHeadCenter;
            private GUIStyle _styleStatus;
            private GUIStyle _styleLegend;
            private GUIStyle _styleButton;
            private GUIStyle _styleProgress;
            private GUIStyle _styleClose;
            private GUIStyle _stylePlaceholder;
            private GUIStyle _styleEmptyTitle;
            private GUIStyle _styleEmptySub;
            private GUIStyle _styleSearch;
            private GUIStyle _styleBlank;
            private GUIStyle _cmHint;
            private GUIStyle _cmDot;

            private bool _scrollDragging;
            private float _scrollDragOffset;
            private bool _inGroup;
            private Vector2 _groupMouse;
            private bool _depthRaised;
            #endregion

            #region CM 探测
            private static bool _cmProbed;
            private static bool _cmAvailable;
            private static Type _cmAttrType;

            internal static bool ConfigurationManagerAvailable
            {
                get
                {
                    ProbeConfigurationManager();
                    return _cmAvailable;
                }
            }

            private static void ProbeConfigurationManager()
            {
                if (_cmProbed) return;
                _cmProbed = true;
                try
                {
                    foreach (var asm in AppDomain.CurrentDomain.GetAssemblies())
                    {
                        var type = asm.GetType("ConfigurationManager.ConfigurationManagerAttributes", false);
                        if (type != null)
                        {
                            _cmAttrType = type;
                            _cmAvailable = true;
                            return;
                        }
                    }

                    // 软依赖不会保证加载顺序，这里主动探测插件目录中的 CM 程序集
                    var loaded = Assembly.Load("BepInEx.ConfigurationManager");
                    var found = loaded.GetType("ConfigurationManager.ConfigurationManagerAttributes", false);
                    if (found != null)
                    {
                        _cmAttrType = found;
                        _cmAvailable = true;
                    }
                }
                catch
                {
                    _cmAvailable = false;
                }
            }

            /// <summary>
            /// 用反射创建 ConfigurationManager 的标签对象：这样未安装 CM 时插件仍可加载，
            /// 也不会在 JIT 阶段因缺少类型而抛 TypeLoadException。
            /// </summary>
            internal static object CreateConfigurationManagerAttributes(Action<ConfigEntryBase> drawer)
            {
                ProbeConfigurationManager();
                if (!_cmAvailable || _cmAttrType == null) return null;

                try
                {
                    var attr = Activator.CreateInstance(_cmAttrType);
                    SetMember(attr, "CustomDrawer", drawer);
                    SetMember(attr, "DispName", "云存档同步面板");
                    SetMember(attr, "Order", -100);
                    SetMember(attr, "Description", "打开游戏内云存档同步面板，手动选择要上传或下载的存档");
                    return attr;
                }
                catch
                {
                    return null;
                }
            }

            private static void SetMember(object target, string name, object value)
            {
                var flags = BindingFlags.Public | BindingFlags.Instance;
                var field = _cmAttrType.GetField(name, flags);
                if (field != null)
                {
                    field.SetValue(target, Coerce(value, field.FieldType));
                    return;
                }
                var prop = _cmAttrType.GetProperty(name, flags);
                if (prop != null && prop.CanWrite)
                {
                    prop.SetValue(target, Coerce(value, prop.PropertyType));
                }
            }

            private static object Coerce(object value, Type type)
            {
                var underlying = Nullable.GetUnderlyingType(type) ?? type;
                if (value == null || underlying.IsInstanceOfType(value)) return value;
                return Convert.ChangeType(value, underlying, CultureInfo.InvariantCulture);
            }
            #endregion

            public SavePanel(DSPRemoteSaves plugin)
            {
                _plugin = plugin;
                // 同时进行的文件数上限，超出的行在“排队中”等待
                _uploadSlots = new SemaphoreSlim(Math.Max(1, plugin.configMaxParallelUploadCount.Value));
                _downloadSlots = new SemaphoreSlim(Math.Max(1, plugin.configMaxParallelDownloadCount.Value));
            }

            #region 生命周期
            internal bool HasActiveTransfers
            {
                get
                {
                    for (int i = 0; i < _rows.Count; i++)
                    {
                        if (_rows[i].IsBusy) return true;
                    }
                    return false;
                }
            }

            internal void Tick()
            {
                if (Input.GetKeyDown(_plugin.configPanelHotkey.Value))
                {
                    if (_visible) Close();
                    else Open();
                }

                if (!_visible) return;

                if (Input.GetKeyDown(KeyCode.Escape))
                {
                    Close();
                    return;
                }

                // 面板是模态的：保持鼠标可见，避免视角被锁定后无法点按钮
                Cursor.visible = true;
                Cursor.lockState = CursorLockMode.None;

                for (int i = 0; i < _rows.Count; i++)
                {
                    if (_rows[i].Status == RowStatus.Running) _rows[i].UpdateSpeed();
                }

                var now = DateTime.UtcNow;
                if (_scheduledRefreshUtc != DateTime.MaxValue && now >= _scheduledRefreshUtc)
                {
                    _scheduledRefreshUtc = DateTime.MaxValue;
                    Refresh();
                }

                var interval = _plugin.configAutoRefreshSeconds.Value;
                if (interval > 0 && !_refreshing && !HasActiveTransfers &&
                    now >= _nextAutoRefreshUtc && !MouseOverPanel())
                {
                    // 鼠标停在面板上时不要重建列表，避免点击瞬间行位置变化
                    Refresh();
                }
            }

            internal void Open()
            {
                if (_visible) return;
                _visible = true;
                _cursorVisible = Cursor.visible;
                _cursorLock = Cursor.lockState;
                _cursorCaptured = true;
                _scroll = 0f;
                _openDropdown = DropdownKind.None;
                Refresh();
            }

            internal void Close()
            {
                if (!_visible) return;
                _visible = false;
                _openDropdown = DropdownKind.None;
                if (_cursorCaptured)
                {
                    Cursor.visible = _cursorVisible;
                    Cursor.lockState = _cursorLock;
                    _cursorCaptured = false;
                }
            }

            internal void Shutdown()
            {
                CancelAll();
                _visible = false;
                if (_cursorCaptured)
                {
                    Cursor.visible = _cursorVisible;
                    Cursor.lockState = _cursorLock;
                    _cursorCaptured = false;
                }
            }

            internal void DrawConfigEntry(ConfigEntryBase entry)
            {
                if (_cmDot == null)
                {
                    var font = EnsureFont();
                    _cmDot = new GUIStyle(GUI.skin.label)
                    {
                        font = font,
                        fontSize = Mathf.Max(10, GUI.skin.label.fontSize)
                    };
                    _cmHint = new GUIStyle(GUI.skin.label)
                    {
                        font = font,
                        fontSize = Mathf.Max(9, GUI.skin.label.fontSize - 2),
                        wordWrap = false,
                        clipping = TextClipping.Clip
                    };
                    _cmHint.normal.textColor = new Color(0.58f, 0.64f, 0.69f, 1f);
                }

                GUILayout.BeginVertical();
                GUILayout.BeginHorizontal();

                var dotColor = _connection == ConnState.Connected ? AccentGreen
                    : _connection == ConnState.Failed ? AccentRed
                    : AccentAmber;
                var prev = GUI.color;
                GUI.color = dotColor;
                GUILayout.Label("●", _cmDot, GUILayout.Width(14));
                GUI.color = prev;
                GUILayout.Label(ConfigSummary, GUILayout.ExpandWidth(true));

                if (GUILayout.Button(_visible ? "面板已打开" : "打开面板", GUILayout.Width(96)))
                {
                    if (_visible) Close();
                    else Open();
                }
                if (GUILayout.Button("刷新", GUILayout.Width(52)))
                {
                    Refresh();
                }

                GUILayout.EndHorizontal();
                GUILayout.Label(
                    $"快捷键 {_plugin.configPanelHotkey.Value} · 本地 {CountLocal()} · 云端 {CountRemote()} · 传输中 {CountBusy()}",
                    _cmHint);
                GUILayout.EndVertical();
            }

            private string ConfigSummary
            {
                get
                {
                    switch (_connection)
                    {
                        case ConnState.Connecting: return "正在连接 " + ServerDisplay + "…";
                        case ConnState.Connected:
                            return $"已连接 · {ServerDisplay} · {_rows.Count} 个存档" +
                                   (HasActiveTransfers ? $" · 传输中 {CountBusy()}" : "");
                        case ConnState.Failed:
                            return "未连接" + (string.IsNullOrEmpty(_lastErrorShort) ? "" : " · " + Trim(_lastErrorShort, 26));
                        default:
                            return "云存档同步：待加载";
                    }
                }
            }

            private string _lastErrorShort;
            #endregion

            #region 数据刷新
            private string ServerDisplay
            {
                get
                {
                    var address = _plugin.httpClient?.BaseAddress;
                    if (address == null) return "未连接";
                    return address.IsDefaultPort ? address.Host : address.Host + ":" + address.Port;
                }
            }

            private int CountLocal()
            {
                int n = 0;
                for (int i = 0; i < _rows.Count; i++) if (_rows[i].LocalExists) n++;
                return n;
            }

            private int CountRemote()
            {
                int n = 0;
                for (int i = 0; i < _rows.Count; i++) if (_rows[i].RemoteExists) n++;
                return n;
            }

            private int CountBusy()
            {
                int n = 0;
                for (int i = 0; i < _rows.Count; i++) if (_rows[i].IsBusy) n++;
                return n;
            }

            private async void Refresh()
            {
                if (_refreshing) return;

                _lastErrorShort = null;
                if (!_plugin.PrepareConnection(out var configError))
                {
                    _connection = ConnState.Failed;
                    _listError = configError;
                    _lastErrorShort = configError;
                    _loading = false;
                    _nextAutoRefreshUtc = DateTime.UtcNow.AddSeconds(5);
                    return;
                }

                _refreshing = true;
                _loading = true;
                _listError = null;
                _connection = ConnState.Connecting;

                try
                {
                    var listingTask = _plugin.GetRemoteFileList();
                    var local = _plugin.GetLocalFileIndex();
                    var listing = await listingTask;

                    if (listing == null)
                    {
                        _connection = ConnState.Failed;
                        _listError = "无法获取云端存档列表" +
                                     (string.IsNullOrEmpty(_plugin.LastServerError) ? "" : "：" + Trim(_plugin.LastServerError, 40)) +
                                     "（详见 BepInEx 日志）";
                        _lastErrorShort = "列表获取失败";
                        _nextAutoRefreshUtc = DateTime.UtcNow.AddSeconds(15);
                        return;
                    }

                    RebuildRows(listing, local);
                    ApplyQuota(listing);
                    _connection = ConnState.Connected;
                    _nextAutoRefreshUtc = DateTime.UtcNow.AddSeconds(
                        Math.Max(10, _plugin.configAutoRefreshSeconds.Value));
                }
                catch (Exception ex)
                {
                    _connection = ConnState.Failed;
                    _listError = ShortError(ex);
                    _lastErrorShort = _listError;
                    _nextAutoRefreshUtc = DateTime.UtcNow.AddSeconds(15);
                    _plugin.Logger.LogError($"刷新云存档列表失败: {_plugin.FormatException(ex)}");
                }
                finally
                {
                    _refreshing = false;
                    _loading = false;
                }
            }

            private void ApplyQuota(RemoteListing listing)
            {
                _hasQuota = listing.HasQuota;
                _quotaBytes = listing.QuotaBytes;
                _usedBytes = listing.UsedBytes;
                _reservedBytes = listing.ReservedBytes;
                _quotaText = _hasQuota ? FormatQuota(_usedBytes + _reservedBytes, _quotaBytes) : "—";
            }

            private void RebuildRows(RemoteListing listing, Dictionary<string, FileInfo> local)
            {
                var existing = new Dictionary<string, SaveRow>(StringComparer.OrdinalIgnoreCase);
                for (int i = 0; i < _rows.Count; i++)
                {
                    existing[_rows[i].FileName] = _rows[i];
                }

                var remoteIndex = new Dictionary<string, RemoteFile>(StringComparer.OrdinalIgnoreCase);
                foreach (var file in listing.Files)
                {
                    if (file == null || string.IsNullOrEmpty(file.FileName)) continue;
                    remoteIndex[file.FileName] = file;
                }

                var keys = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                foreach (var key in remoteIndex.Keys) keys.Add(key);
                foreach (var key in local.Keys) keys.Add(key);

                var rebuilt = new List<SaveRow>(keys.Count);
                foreach (var key in keys)
                {
                    remoteIndex.TryGetValue(key, out var remote);
                    local.TryGetValue(key, out var info);

                    if (existing.TryGetValue(key, out var row))
                    {
                        // 正在传输的行保留进度与取消句柄；其余行原地更新，
                        // 这样“刚上传完”的标记不会因为一次刷新而丢失
                        if (!row.IsBusy) row.UpdateFrom(remote, info);
                        rebuilt.Add(row);
                    }
                    else
                    {
                        rebuilt.Add(new SaveRow(remote, info));
                    }
                }

                _rows.Clear();
                _rows.AddRange(rebuilt);
                RebuildView();
            }

            private void RebuildView()
            {
                IEnumerable<SaveRow> query = _rows;

                if (!string.IsNullOrEmpty(_search))
                {
                    var needle = _search.Trim();
                    query = query.Where(r =>
                        r.DisplayName.IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0 ||
                        r.FileName.IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0);
                }

                switch (_filter)
                {
                    case DirectionFilter.Upload:
                        query = query.Where(r => r.Direction == RowDirection.Upload);
                        break;
                    case DirectionFilter.Download:
                        query = query.Where(r => r.Direction == RowDirection.Download);
                        break;
                    case DirectionFilter.Synced:
                        query = query.Where(r => r.Direction == RowDirection.Synced);
                        break;
                }

                switch (_sort)
                {
                    case SortMode.Name:
                        query = query.OrderBy(r => r.DisplayName, StringComparer.CurrentCultureIgnoreCase);
                        break;
                    case SortMode.Size:
                        query = query.OrderByDescending(r => r.MaxSize);
                        break;
                    default:
                        query = query.OrderByDescending(r => r.RecentUtc);
                        break;
                }

                _view.Clear();
                _view.AddRange(query);
            }

            private void ScheduleRefresh(float seconds)
            {
                _scheduledRefreshUtc = DateTime.UtcNow.AddSeconds(seconds);
            }
            #endregion

            #region 传输调度
            private void StartTransfer(SaveRow row, TransferKind kind)
            {
                if (row == null || row.IsBusy) return;
                if (kind == TransferKind.Upload && !row.LocalExists) return;
                if (kind == TransferKind.Download && !row.RemoteExists) return;

                if (!_plugin.PrepareConnection(out var error))
                {
                    row.Fail(error);
                    return;
                }

                row.Begin(kind);
                _ = RunTransferAsync(row, kind);
            }

            private async Task RunTransferAsync(SaveRow row, TransferKind kind)
            {
                var started = DateTime.UtcNow;
                var token = row.Cts.Token;
                var slots = kind == TransferKind.Upload ? _uploadSlots : _downloadSlots;
                var slotTaken = false;

                try
                {
                    await slots.WaitAsync(token);
                    slotTaken = true;
                    row.MarkRunning();

                    if (kind == TransferKind.Upload)
                    {
                        // 以磁盘上的当前时间为准，行里的 mtime 可能来自上次刷新
                        var mtimeBefore = row.LocalMtimeUtc;
                        try
                        {
                            if (File.Exists(row.LocalPath)) mtimeBefore = File.GetLastWriteTimeUtc(row.LocalPath);
                        }
                        catch { }

                        var ok = await _plugin.UploadSingleFile(row.LocalPath, row.AddProgress, token);
                        if (!ok)
                        {
                            if (!token.IsCancellationRequested)
                            {
                                row.Fail(string.IsNullOrEmpty(_plugin.LastServerError)
                                    ? "上传失败，详见 BepInEx 日志"
                                    : "上传失败：" + Trim(_plugin.LastServerError, 52));
                            }
                            return;
                        }

                        row.FillProgress();

                        // 服务端把 mtime 记为提交时间。内容已经一致，把本地 mtime 对齐到云端，
                        // 否则刷新后会显示“云端较新”，甚至被误判为需要再次下载。
                        var listing = await _plugin.GetRemoteFileList();

                        // 上传期间游戏可能又写了一次存档；只有本地完全没变，
                        // 才能把本地时间对齐到云端并标记“已一致”。
                        var localUnchanged = false;
                        try
                        {
                            localUnchanged = File.Exists(row.LocalPath) &&
                                             File.GetLastWriteTimeUtc(row.LocalPath) == mtimeBefore;
                        }
                        catch { }
                        var remote = listing?.Files?.FirstOrDefault(f =>
                            string.Equals(f.FileName, row.FileName, StringComparison.OrdinalIgnoreCase));
                        if (remote != null)
                        {
                            try
                            {
                                if (localUnchanged && remote.LastModified > 0 && File.Exists(row.LocalPath))
                                {
                                    File.SetLastWriteTimeUtc(
                                        row.LocalPath,
                                        DateTimeOffset.FromUnixTimeMilliseconds(remote.LastModified).UtcDateTime);
                                }
                            }
                            catch (Exception ex)
                            {
                                _plugin.Logger.LogWarning($"对齐本地存档时间失败（{row.FileName}）: {ex.Message}");
                            }

                            row.ApplyRemote(remote);
                            row.RefreshLocal();
                            ApplyQuota(listing);
                        }

                        if (localUnchanged) row.MarkUploaded();
                        row.Complete();
                        _plugin.Logger.LogInfo(
                            $"已上传 {row.FileName}（{FormatSize(row.LocalSize)}，{Seconds(started):0.0}s）");
                    }
                    else
                    {
                        var remote = row.Remote ?? new RemoteFile
                        {
                            FileName = row.FileName,
                            FileSize = row.RemoteSize,
                            LastModified = row.RemoteMtimeMs
                        };

                        var ok = await _plugin.DownloadSingleFile(remote, row.AddProgress, token);
                        if (!ok)
                        {
                            if (!token.IsCancellationRequested)
                            {
                                row.Fail(string.IsNullOrEmpty(_plugin.LastServerError)
                                    ? "下载失败，详见 BepInEx 日志"
                                    : "下载失败：" + Trim(_plugin.LastServerError, 52));
                            }
                            return;
                        }

                        row.FillProgress();
                        row.RefreshLocal();
                        row.Complete();
                        _plugin.Logger.LogInfo(
                            $"已下载 {row.FileName}（{FormatSize(row.LocalSize)}，{Seconds(started):0.0}s）");
                    }
                }
                catch (OperationCanceledException)
                {
                    row.Canceled();
                }
                catch (Exception ex)
                {
                    row.Fail(ShortError(ex));
                    _plugin.Logger.LogError($"传输失败 {row.FileName}: {_plugin.FormatException(ex)}");
                }
                finally
                {
                    if (slotTaken) slots.Release();
                    row.End();
                    RebuildView();
                    ScheduleRefresh(0.6f);
                }
            }

            private void CancelRow(SaveRow row)
            {
                if (row == null || !row.IsBusy) return;
                try { row.Cts?.Cancel(); }
                catch (ObjectDisposedException) { }
            }

            private void CancelAll()
            {
                for (int i = 0; i < _rows.Count; i++)
                {
                    if (_rows[i].IsBusy) CancelRow(_rows[i]);
                }
            }

            private double Seconds(DateTime startedUtc)
            {
                return Math.Max(0.0, (DateTime.UtcNow - startedUtc).TotalSeconds);
            }
            #endregion

            #region 绘制
            private bool _blockInput;

            internal void Draw()
            {
                if (!_visible)
                {
                    if (_depthRaised)
                    {
                        GUI.depth = 0;
                        _depthRaised = false;
                    }
                    return;
                }

                // 让面板盖在 Configuration Manager 等其它 IMGUI 窗口之上
                GUI.depth = -100;
                _depthRaised = true;
                _scale = ComputeScale();
                EnsureAssets();

                // 部分游戏逻辑会在 Update 里重新锁定鼠标，OnGUI 阶段再兜底一次
                Cursor.visible = true;
                Cursor.lockState = CursorLockMode.None;

                _px = (Screen.width - PanelW * _scale) * 0.5f;
                _py = (Screen.height - PanelH * _scale) * 0.5f;
                _blockInput = _openDropdown != DropdownKind.None;

                DrawRect(new Rect(0, 0, Screen.width, Screen.height), new Color(0f, 0f, 0f, 0.42f));

                DrawRounded(
                    new Rect(_px - S(3), _py + S(4), S(PanelW) + S(6), S(PanelH) + S(4)),
                    new Color(0f, 0f, 0f, 0.35f), S(Radius + 2), null, 0);

                DrawRounded(R(0, 0, PanelW, PanelH), PanelBg, S(Radius), Border, S(1));
                DrawHeader();
                DrawToolbar();
                DrawTableHead();
                DrawList();
                DrawFooter();

                // 内容会盖住面板初始边框，最后补一圈直线（圆角处留空）
                DrawRect(R(Radius, 0, PanelW - Radius * 2, 1), Border);
                DrawRect(R(Radius, PanelH - 1, PanelW - Radius * 2, 1), Border);
                DrawRect(R(0, Radius, 1, PanelH - Radius * 2), Border);
                DrawRect(R(PanelW - 1, Radius, 1, PanelH - Radius * 2), Border);

                DrawDropdown();
            }

            private float ComputeScale()
            {
                var byHeight = Screen.height / 1080f;
                var fitWidth = Screen.width * 0.96f / PanelW;
                var fitHeight = Screen.height * 0.94f / PanelH;
                return Mathf.Clamp(Mathf.Min(byHeight, Mathf.Min(fitWidth, fitHeight)), 0.62f, 2.6f);
            }

            private Font EnsureFont()
            {
                if (_font != null) return _font;
                try
                {
                    _font = Font.CreateDynamicFontFromOSFont(
                        new[] { "Microsoft YaHei UI", "Microsoft YaHei", "SimHei", "Noto Sans CJK SC", "Arial" }, 18);
                }
                catch { }
                if (_font == null) _font = GUI.skin.font;
                return _font;
            }

            private void EnsureAssets()
            {
                EnsureFont();

                if (_white == null)
                {
                    _white = new Texture2D(1, 1, TextureFormat.RGBA32, false);
                    _white.SetPixel(0, 0, Color.white);
                    _white.Apply();
                    _white.hideFlags = HideFlags.HideAndDontSave;
                }
                if (_logo == null) _logo = MakeLogoTexture();
                if (_magnifier == null) _magnifier = MakeMagnifierTexture();

                try
                {
                    GUI.skin.settings.cursorColor = AccentCyan;
                    GUI.skin.settings.selectionColor = new Color(0.24f, 0.55f, 0.68f, 0.55f);
                }
                catch { }

                if (Mathf.Abs(_builtScale - _scale) < 0.001f) return;

                _builtScale = _scale;
                _styleTitle = MakeStyle(16f, FontStyle.Bold, TextAnchor.MiddleLeft, TextPrimary);
                _styleSubtitle = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleLeft, TextSecondary);
                _styleRowName = MakeStyle(13.5f, FontStyle.Normal, TextAnchor.MiddleLeft, TextPrimary);
                _styleMetaRight = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleRight, TextSecondary);
                _styleRight = MakeStyle(12.5f, FontStyle.Normal, TextAnchor.MiddleRight, TextPrimary);
                _styleSmallRight = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleRight, TextSecondary);
                _styleSmallPrimary = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleLeft, TextPrimary);
                _styleDimSmall = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleLeft, TextDim);
                _styleTableHead = MakeStyle(10.5f, FontStyle.Normal, TextAnchor.MiddleLeft, TextDim);
                _styleTableHeadRight = MakeStyle(10.5f, FontStyle.Normal, TextAnchor.MiddleRight, TextDim);
                _styleTableHeadCenter = MakeStyle(10.5f, FontStyle.Normal, TextAnchor.MiddleCenter, TextDim);
                _styleStatus = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleLeft, Color.white);
                _styleLegend = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleLeft, Color.white);
                _styleButton = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleCenter, Color.white);
                _styleProgress = MakeStyle(10.5f, FontStyle.Normal, TextAnchor.MiddleLeft, Color.white);
                _styleClose = MakeStyle(18f, FontStyle.Normal, TextAnchor.MiddleCenter, Color.white);
                _stylePlaceholder = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleLeft, TextDim);
                _styleEmptyTitle = MakeStyle(15f, FontStyle.Normal, TextAnchor.MiddleCenter, TextPrimary);
                _styleEmptySub = MakeStyle(11.5f, FontStyle.Normal, TextAnchor.MiddleCenter, TextDim);
                _styleBlank = new GUIStyle(GUIStyle.none);

                BuildSearchStyle();
            }

            private GUIStyle MakeStyle(float size, FontStyle fontStyle, TextAnchor anchor, Color color)
            {
                var style = new GUIStyle(GUI.skin.label)
                {
                    font = _font,
                    fontSize = Mathf.Max(9, Mathf.RoundToInt(size * _scale)),
                    fontStyle = fontStyle,
                    alignment = anchor,
                    clipping = TextClipping.Clip,
                    wordWrap = false,
                    richText = false,
                    padding = new RectOffset(0, 0, 0, 0),
                    margin = new RectOffset(0, 0, 0, 0)
                };
                style.normal.textColor = color;
                style.hover.textColor = color;
                style.active.textColor = color;
                style.focused.textColor = color;
                return style;
            }

            private void BuildSearchStyle()
            {
                var radius = Mathf.Max(3, Mathf.RoundToInt(6 * _scale));
                if (_searchBgTex != null)
                {
                    UnityEngine.Object.Destroy(_searchBgTex);
                    _searchBgTex = null;
                }
                _searchBgTex = MakeRoundedBoxTexture(radius, C(8, 24, 35, 1f), C(41, 78, 99, 1f), Mathf.Max(1, Mathf.RoundToInt(_scale)));
                _styleSearch = new GUIStyle(GUI.skin.textField)
                {
                    font = _font,
                    fontSize = Mathf.Max(9, Mathf.RoundToInt(12f * _scale)),
                    alignment = TextAnchor.MiddleLeft,
                    clipping = TextClipping.Clip,
                    border = new RectOffset(radius, radius, radius, radius),
                    padding = new RectOffset(Mathf.RoundToInt(26 * _scale), Mathf.RoundToInt(8 * _scale), 0, 0)
                };
                _styleSearch.normal.background = _searchBgTex;
                _styleSearch.hover.background = _searchBgTex;
                _styleSearch.active.background = _searchBgTex;
                _styleSearch.focused.background = _searchBgTex;
                _styleSearch.normal.textColor = TextPrimary;
                _styleSearch.hover.textColor = TextPrimary;
                _styleSearch.active.textColor = TextPrimary;
                _styleSearch.focused.textColor = TextPrimary;
            }

            private void DrawHeader()
            {
                var header = new Rect(_px + S(1), _py + S(1), S(PanelW) - S(2), S(HeaderH));
                DrawRounded(header, HeaderBg, S(Radius - 1), null, 0);
                DrawRect(new Rect(header.x, header.y + S(HeaderH - Radius), header.width, S(Radius)), HeaderBg);
                DrawRect(new Rect(header.x + S(1), header.y + header.height - S(1), header.width - S(2), S(1)),
                    C(120, 170, 195, 0.22f));

                GUI.DrawTexture(R(18, 11, 22, 22), _logo);
                GUI.Label(R(48, 0, 240, HeaderH), "DSP Remote Saves", _styleTitle);

                var titleWidth = _styleTitle.CalcSize(new GUIContent("DSP Remote Saves")).x / _scale;
                GUI.Label(R(48 + titleWidth + 12, 0, 140, HeaderH), "云存档同步", _styleSubtitle);

                var status = StatusLine;
                var statusWidth = _styleStatus.CalcSize(new GUIContent(status)).x / _scale;
                var statusX = 1110f - statusWidth;
                DrawCircle(R(statusX - 11, HeaderH * 0.5f - 2.5f, 5, 5), StatusColor);
                var prev = GUI.color;
                GUI.color = _connection == ConnState.Failed ? AccentRed : TextSecondary;
                GUI.Label(R(statusX, 0, statusWidth + 2, HeaderH), status, _styleStatus);
                GUI.color = prev;

                var close = R(1122, 9, 26, 26);
                if (close.Contains(Event.current.mousePosition) && !_blockInput)
                {
                    DrawRounded(close, C(255, 255, 255, 0.08f), S(5), null, 0);
                }
                GUI.color = C(190, 210, 222, 1f);
                GUI.Label(close, "×", _styleClose);
                GUI.color = prev;
                if (Click(close)) Close();
            }

            private string StatusLine
            {
                get
                {
                    switch (_connection)
                    {
                        case ConnState.Connecting: return "连接中 · " + ServerDisplay;
                        case ConnState.Connected: return "已连接 · " + ServerDisplay;
                        case ConnState.Failed: return "未连接 · " + ServerDisplay;
                        default: return "未连接";
                    }
                }
            }

            private Color StatusColor
            {
                get
                {
                    switch (_connection)
                    {
                        case ConnState.Connected: return AccentGreen;
                        case ConnState.Connecting: return AccentAmber;
                        case ConnState.Failed: return AccentRed;
                        default: return TextFaint;
                    }
                }
            }

            private void DrawToolbar()
            {
                DrawRect(R(0, HeaderH, PanelW, ToolbarH), ToolbarBg);
                DrawRect(R(0, HeaderH + ToolbarH - 1, PanelW, 1), Separator);

                GUI.Label(R(18, HeaderH, 30, ToolbarH), "排序", _styleDimSmall);
                GUI.Label(R(50, HeaderH, 70, ToolbarH), SortLabel, _styleSmallPrimary);
                var prev = GUI.color;
                GUI.color = TextDim;
                GUI.Label(R(112, HeaderH, 14, ToolbarH), "▾", _styleLegend);
                GUI.color = prev;
                if (Click(R(46, HeaderH + 6, 82, ToolbarH - 12))) ToggleDropdown(DropdownKind.Sort);

                DrawRect(R(140, HeaderH + 10, 1, ToolbarH - 20), Separator);

                GUI.Label(R(156, HeaderH, 60, ToolbarH), "传输方向", _styleDimSmall);
                GUI.Label(R(214, HeaderH, 48, ToolbarH), FilterLabel, _styleSmallPrimary);
                GUI.color = TextDim;
                GUI.Label(R(260, HeaderH, 14, ToolbarH), "▾", _styleLegend);
                GUI.color = prev;
                if (Click(R(210, HeaderH + 6, 66, ToolbarH - 12))) ToggleDropdown(DropdownKind.Filter);

                // 刷新
                var refresh = R(662, HeaderH + 7, 52, ToolbarH - 14);
                if (refresh.Contains(Event.current.mousePosition) && !_blockInput)
                {
                    DrawRounded(refresh, C(255, 255, 255, 0.06f), S(4), null, 0);
                }
                GUI.color = _refreshing ? TextFaint : TextSecondary;
                GUI.Label(refresh, _refreshing ? "刷新中" : "刷新", _styleLegend);
                GUI.color = prev;
                if (Click(refresh) && !_refreshing) Refresh();

                // 云空间
                var quota = _hasQuota ? "云空间 " + _quotaText : "云空间 —";
                GUI.Label(R(716, HeaderH, 216, ToolbarH), quota, _styleSmallRight);

                // 搜索框
                var field = R(946, HeaderH + 7, 198, 26);
                var focused = GUI.GetNameOfFocusedControl() == "dsprs-search";
                GUI.SetNextControlName("dsprs-search");
                var next = GUI.TextField(field, _search ?? string.Empty, 80, _styleSearch);
                if (next != _search)
                {
                    _search = next;
                    _scroll = 0f;
                    RebuildView();
                }
                GUI.DrawTexture(R(952, HeaderH + 14, 13, 13), _magnifier);
                if (string.IsNullOrEmpty(_search) && !focused)
                {
                    GUI.Label(R(972, HeaderH, 160, ToolbarH), "搜索存档名称", _stylePlaceholder);
                }
            }

            private string SortLabel
            {
                get
                {
                    switch (_sort)
                    {
                        case SortMode.Name: return "名称";
                        case SortMode.Size: return "大小";
                        default: return "最近修改";
                    }
                }
            }

            private string FilterLabel
            {
                get
                {
                    switch (_filter)
                    {
                        case DirectionFilter.Upload: return "待上传";
                        case DirectionFilter.Download: return "待下载";
                        case DirectionFilter.Synced: return "已一致";
                        default: return "全部";
                    }
                }
            }

            private void DrawTableHead()
            {
                var y = HeaderH + ToolbarH;
                DrawRect(R(0, y, PanelW, TableHeadH), TableHeadBg);
                GUI.Label(R(18, y, 200, TableHeadH), "名称", _styleTableHead);
                GUI.Label(R(460, y, 240, TableHeadH), "本地版本", _styleTableHeadRight);
                GUI.Label(R(720, y, 240, TableHeadH), "同步方向", _styleTableHeadCenter);
                GUI.Label(R(900, y, 244, TableHeadH), "云端版本", _styleTableHeadRight);
                DrawRect(R(0, y + TableHeadH - 1, PanelW, 1), Separator);
            }

            private void DrawList()
            {
                var view = R(0, ListY, PanelW, ListH);

                var contentHeight = 0f;
                for (int i = 0; i < _view.Count; i++)
                {
                    contentHeight += S(RowH);
                    if (_view[i].IsBusy) contentHeight += S(ProgressH);
                    if (_view[i].Status == RowStatus.Failed) contentHeight += S(ErrorH);
                }
                _contentHeight = contentHeight;

                if (view.Contains(Event.current.mousePosition) && Event.current.type == EventType.ScrollWheel)
                {
                    _scroll += Event.current.delta.y * S(48);
                    Event.current.Use();
                }
                _scroll = Mathf.Clamp(_scroll, 0f, Mathf.Max(0f, contentHeight - view.height));

                _groupMouse = LocalMouse(view);
                _inGroup = true;
                GUI.BeginGroup(view);
                if (_view.Count == 0)
                {
                    DrawEmptyState();
                }
                else
                {
                    var y = -_scroll;
                    for (int i = 0; i < _view.Count; i++)
                    {
                        var row = _view[i];
                        var h = S(RowH);
                        var busy = row.IsBusy;
                        var failed = row.Status == RowStatus.Failed;
                        if (y + h >= 0 && y <= view.height)
                        {
                            var hover = !_blockInput &&
                                        new Rect(0, y, view.width, h).Contains(LocalMouse(view));
                            DrawRow(row, i, y, hover);
                        }
                        y += h;
                        if (busy) y += S(ProgressH);
                        if (failed) y += S(ErrorH);
                    }
                }
                GUI.EndGroup();
                _inGroup = false;

                DrawScrollbar(view, contentHeight);
            }

            private bool MouseOverPanel()
            {
                if (_scale <= 0f) return false;
                var mouse = Input.mousePosition;
                var point = new Vector2(mouse.x, Screen.height - mouse.y);
                return new Rect(_px, _py, PanelW * _scale, PanelH * _scale).Contains(point);
            }

            private Vector2 Pointer
            {
                get { return _inGroup ? _groupMouse : Event.current.mousePosition; }
            }

            private Vector2 LocalMouse(Rect view)
            {
                var mouse = Event.current.mousePosition;
                return new Vector2(mouse.x - view.x, mouse.y - view.y);
            }

            private void DrawEmptyState()
            {
                string title;
                string subtitle;
                var retry = false;

                if (_loading)
                {
                    title = "正在获取云端存档…";
                    subtitle = "连接 " + ServerDisplay;
                }
                else if (!string.IsNullOrEmpty(_listError))
                {
                    title = "连接失败";
                    subtitle = _listError;
                    retry = true;
                }
                else if (_rows.Count == 0)
                {
                    title = "还没有可同步的存档";
                    subtitle = "本地存档会自动出现在这里，点击上传即可备份到云端";
                }
                else
                {
                    title = "没有匹配的存档";
                    subtitle = "试试其它关键字，或把筛选切回“全部”";
                }

                GUI.Label(L(0, ListH * 0.5f - 44, PanelW, 26),
                    title, _styleEmptyTitle);
                GUI.Label(L(0, ListH * 0.5f - 12, PanelW, 22),
                    Trim(subtitle, 56), _styleEmptySub);

                if (retry)
                {
                    var button = L(PanelW * 0.5f - 50, ListH * 0.5f + 22, 100, 28);
                    if (DrawSecondaryButton(button, "重试")) Refresh();
                }
            }

            private void DrawScrollbar(Rect view, float contentHeight)
            {
                if (contentHeight <= view.height + 1f) return;

                var track = new Rect(view.xMax - S(6), view.y + S(4), S(3), view.height - S(8));
                DrawRounded(track, C(255, 255, 255, 0.06f), S(2), null, 0);

                var thumbHeight = Mathf.Max(S(26), track.height * (view.height / contentHeight));
                var maxScroll = Mathf.Max(1f, contentHeight - view.height);
                var t = Mathf.Clamp01(_scroll / maxScroll);
                var thumb = new Rect(track.x, track.y + (track.height - thumbHeight) * t, track.width, thumbHeight);
                DrawRounded(thumb, C(190, 220, 235, 0.32f), S(2), null, 0);

                var mouse = Event.current.mousePosition;
                if (Event.current.type == EventType.MouseDown && thumb.Contains(mouse))
                {
                    _scrollDragging = true;
                    _scrollDragOffset = mouse.y - thumb.y;
                    Event.current.Use();
                }
                else if (_scrollDragging && Event.current.type == EventType.MouseDrag)
                {
                    var y = mouse.y - _scrollDragOffset - track.y;
                    var ratio = Mathf.Clamp01(y / Mathf.Max(1f, track.height - thumbHeight));
                    _scroll = ratio * maxScroll;
                    Event.current.Use();
                }
                else if (Event.current.type == EventType.MouseUp)
                {
                    _scrollDragging = false;
                }
            }

            private void DrawRow(SaveRow row, int index, float y, bool hover)
            {
                var busy = row.IsBusy;
                var failed = row.Status == RowStatus.Failed;
                var height = S(RowH);

                var background = index % 2 == 0 ? RowBgA : RowBgB;
                if (hover) background = RowHover;
                DrawRect(L(0, y, PanelW, RowH), background);

                // 左侧状态点
                var dot = L(19, y + RowH * 0.5f - 4, 8, 8);
                var dotColor = busy ? (row.Kind == TransferKind.Upload ? AccentCyan : AccentAmber)
                    : failed ? AccentRed
                    : row.Direction == RowDirection.Synced ? C(86, 132, 152, 1f)
                    : C(118, 158, 178, 1f);
                DrawCircle(dot, dotColor);
                DrawCircle(L(21, y + RowH * 0.5f - 2, 4, 4), background);

                GUI.Label(L(34, y, 430, RowH), Trim(row.DisplayName, 30), _styleRowName);

                // 本地版本
                if (row.LocalExists)
                {
                    GUI.Label(L(462, y, 138, RowH), FormatSize(row.LocalSize), _styleRight);
                    GUI.Label(L(606, y, 96, RowH), FormatTime(row.LocalMtimeUtc.ToLocalTime()), _styleMetaRight);
                }
                else
                {
                    DrawDashedPlaceholder(L(462, y + 15, 240, RowH - 30), "无本地存档");
                }

                // 同步方向
                var dy = y + 15;
                var dh = RowH - 30;
                if (row.Direction == RowDirection.Synced && !busy && !failed)
                {
                    DrawSyncChip(L(790, dy, 92, dh));
                }
                else
                {
                    var primary = row.Direction == RowDirection.Download
                        ? TransferKind.Download
                        : TransferKind.Upload;

                    // 传输中按当前方向显示，按钮置灰
                    if (busy) primary = row.Kind;

                    // 位置固定：上传在左、下载在右（与定稿一致），只有样式随“主方向”变化
                    var uploadEnabled = row.LocalExists && !busy && !_blockInput;
                    var downloadEnabled = row.RemoteExists && !busy && !_blockInput;
                    var uploadRect = L(736, dy, 76, dh);
                    var downloadRect = L(820, dy, 76, dh);

                    if (primary == TransferKind.Download)
                    {
                        DrawActionLink(uploadRect, "上传 →", uploadEnabled,
                            () => StartTransfer(row, TransferKind.Upload));
                        DrawActionButton(downloadRect, "← 下载", true, downloadEnabled,
                            () => StartTransfer(row, TransferKind.Download));
                    }
                    else
                    {
                        DrawActionButton(uploadRect, "上传 →", false, uploadEnabled,
                            () => StartTransfer(row, TransferKind.Upload));
                        DrawActionLink(downloadRect, "← 下载", downloadEnabled,
                            () => StartTransfer(row, TransferKind.Download));
                    }
                }

                // 云端版本
                if (row.RemoteExists)
                {
                    GUI.Label(L(950, y, 92, RowH), FormatSize(row.RemoteSize), _styleRight);
                    GUI.Label(L(1050, y, 94, RowH), FormatTime(FromUnixMs(row.RemoteMtimeMs).ToLocalTime()), _styleMetaRight);
                }
                else
                {
                    DrawDashedPlaceholder(L(938, y + 15, 206, RowH - 30), "无云端存档");
                }

                DrawRect(L(16, y + RowH - 1, PanelW - 32, 1), Separator);

                if (busy) DrawProgressLine(row, y + RowH);
                else if (failed) DrawErrorLine(row, y + RowH);
            }

            private void DrawProgressLine(SaveRow row, float y)
            {
                var upload = row.Kind == TransferKind.Upload;
                var queued = row.Status == RowStatus.Queued;
                var accent = upload ? AccentCyan : AccentAmber;
                DrawRect(L(0, y, PanelW, ProgressH), upload ? C(13, 41, 54, 0.85f) : C(43, 34, 23, 0.85f));
                DrawRect(L(16, y, PanelW - 32, 1), Separator);

                var prev = GUI.color;
                GUI.color = accent;
                GUI.Label(L(18, y, 100, ProgressH), queued ? (upload ? "↑ 排队中" : "← 排队中")
                    : (upload ? "↑ 上传中" : "← 下载中"), _styleProgress);
                GUI.color = prev;

                if (queued)
                {
                    GUI.Label(L(120, y, 360, ProgressH), "等待其他传输完成…", _styleProgress);
                }
                else
                {
                    var bar = L(120, y + ProgressH * 0.5f - 2, 548, 4);
                    DrawRounded(bar, ProgressTrack, S(2), null, 0);
                    var progress = row.Progress;
                    if (progress > 0f)
                    {
                        DrawRounded(new Rect(bar.x, bar.y, Mathf.Max(S(2), bar.width * progress), bar.height),
                            upload ? ProgressUp : ProgressDown, S(2), null, 0);
                    }

                    var eta = FormatEta(row.EtaSeconds);
                    var text = $"{progress * 100f:0}%   {FormatSize(row.Transferred)} / {FormatSize(row.Total)}   {eta}";
                    GUI.Label(L(684, y, 370, ProgressH), text, _styleProgress);
                }

                var cancel = L(1070, y + 4, 58, ProgressH - 8);
                if (DrawSecondaryButton(cancel, "取消")) CancelRow(row);
            }

            private void DrawErrorLine(SaveRow row, float y)
            {
                DrawRect(L(0, y, PanelW, ErrorH), C(55, 25, 24, 0.85f));
                DrawRect(L(16, y, PanelW - 32, 1), C(120, 60, 52, 0.6f));
                var prev = GUI.color;
                GUI.color = AccentRed;
                GUI.Label(L(18, y, PanelW - 160, ErrorH), "! " + Trim(row.Error ?? "传输失败", 70), _styleProgress);
                GUI.color = prev;
            }

            private void DrawSyncChip(Rect rect)
            {
                DrawRounded(rect, ChipBg, S(4), ChipBorder, S(1));
                var prev = GUI.color;
                GUI.color = AccentCyan;
                GUI.Label(rect, "√ 已一致", _styleButton);
                GUI.color = prev;
            }

            private void DrawFooter()
            {
                var y = PanelH - FooterH;
                var foot = new Rect(_px + S(1), _py + S(y), S(PanelW) - S(2), S(FooterH) - S(1));
                DrawRounded(foot, FooterBg, S(Radius - 1), null, 0);
                DrawRect(new Rect(foot.x, foot.y, foot.width, S(Radius)), FooterBg);
                DrawRect(new Rect(foot.x, foot.y, foot.width, S(1)), Separator);

                var prev = GUI.color;
                GUI.color = AccentCyan;
                GUI.Label(R(18, y, 104, FooterH), "→ 上传到云端", _styleLegend);
                GUI.color = AccentAmber;
                GUI.Label(R(130, y, 104, FooterH), "← 下载到本地", _styleLegend);
                GUI.color = TextFaint;
                GUI.Label(R(244, y, 520, FooterH), "点箭头即刻开始 · 进度在该行下方展开 · 可随时取消，不支持暂停", _styleLegend);
                GUI.color = prev;

                var busy = CountBusy();
                GUI.Label(R(920, y, 100, FooterH), $"正在执行 {busy} 项", _styleSmallRight);

                var cancel = R(1032, y + 9, 112, FooterH - 18);
                if (DrawSecondaryButton(cancel, "全部取消") && busy > 0) CancelAll();
            }

            private void DrawDropdown()
            {
                if (_openDropdown == DropdownKind.None) return;

                string[] items;
                int current;
                float x;
                if (_openDropdown == DropdownKind.Sort)
                {
                    items = new[] { "最近修改", "名称", "大小" };
                    current = (int)_sort;
                    x = 48;
                }
                else
                {
                    items = new[] { "全部", "待上传", "待下载", "已一致" };
                    current = (int)_filter;
                    x = 198;
                }

                var itemH = 28f;
                var box = R(x, HeaderH + ToolbarH - 2, 132, items.Length * itemH + 8);
                var popupRect = box;

                DrawRounded(box, C(15, 39, 54, 0.99f), S(6), Border, S(1));

                var mouse = Event.current.mousePosition;
                for (int i = 0; i < items.Length; i++)
                {
                    var item = new Rect(box.x + S(4), box.y + S(4) + i * S(itemH), box.width - S(8), S(itemH));
                    var hover = item.Contains(mouse);
                    if (hover) DrawRounded(item, C(38, 78, 99, 0.8f), S(4), null, 0);

                    var prev = GUI.color;
                    GUI.color = i == current ? AccentCyan : (hover ? TextPrimary : TextSecondary);
                    GUI.Label(new Rect(item.x + S(10), item.y, item.width - S(16), item.height), items[i], _styleLegend);
                    GUI.color = prev;

                    if (hover && Event.current.type == EventType.MouseDown)
                    {
                        if (_openDropdown == DropdownKind.Sort) _sort = (SortMode)i;
                        else _filter = (DirectionFilter)i;
                        _openDropdown = DropdownKind.None;
                        _scroll = 0f;
                        RebuildView();
                        Event.current.Use();
                        return;
                    }
                }

                if (Event.current.type == EventType.MouseDown && !popupRect.Contains(mouse))
                {
                    _openDropdown = DropdownKind.None;
                    Event.current.Use();
                }
            }

            private void ToggleDropdown(DropdownKind kind)
            {
                _openDropdown = _openDropdown == kind ? DropdownKind.None : kind;
            }

            private bool DrawActionButton(Rect rect, string text, bool download, bool enabled, Action action)
            {
                var hover = enabled && rect.Contains(Pointer);
                var fill = download ? BtnDownloadBg : BtnUploadBg;
                var border = download ? BtnDownloadBorder : BtnUploadBorder;
                var textColor = download ? C(255, 228, 186, 1f) : C(198, 242, 252, 1f);

                if (enabled)
                {
                    if (hover)
                    {
                        fill = download ? C(88, 66, 37, 0.98f) : C(24, 79, 100, 0.98f);
                        border = download ? AccentAmber : AccentCyan;
                    }
                    DrawRounded(rect, fill, S(4), border, S(1));
                }
                else
                {
                    DrawRounded(rect, C(255, 255, 255, 0.03f), S(4), C(80, 104, 118, 0.4f), S(1));
                    textColor = TextFaint;
                }

                var prev = GUI.color;
                GUI.color = textColor;
                GUI.Label(rect, text, _styleButton);
                GUI.color = prev;

                if (enabled && Click(rect))
                {
                    action?.Invoke();
                    return true;
                }
                return false;
            }

            private void DrawActionLink(Rect rect, string text, bool enabled, Action action)
            {
                var hover = enabled && rect.Contains(Pointer);
                var prev = GUI.color;
                GUI.color = enabled ? (hover ? TextPrimary : TextSecondary) : TextFaint;
                GUI.Label(rect, text, _styleButton);
                GUI.color = prev;

                if (enabled && hover)
                {
                    var width = _styleButton.CalcSize(new GUIContent(text)).x;
                    var x = rect.x + (rect.width - width) * 0.5f;
                    DrawRect(new Rect(x, rect.y + rect.height - S(3), width, S(1)), TextSecondary);
                }

                if (enabled && Click(rect)) action?.Invoke();
            }

            private bool DrawSecondaryButton(Rect rect, string text)
            {
                var hover = !_blockInput && rect.Contains(Pointer);
                DrawRounded(rect, hover ? C(39, 76, 96, 0.85f) : C(255, 255, 255, 0.04f), S(4),
                    hover ? C(96, 148, 172, 0.9f) : C(72, 103, 120, 0.6f), S(1));

                var prev = GUI.color;
                GUI.color = hover ? TextPrimary : TextSecondary;
                GUI.Label(rect, text, _styleButton);
                GUI.color = prev;

                return Click(rect);
            }

            private void DrawDashedPlaceholder(Rect rect, string text)
            {
                DrawRect(rect, C(255, 255, 255, 0.025f));
                DrawDashedRect(rect, C(120, 150, 166, 0.5f), 4f, 3f, S(1));
                var prev = GUI.color;
                GUI.color = TextFaint;
                GUI.Label(rect, text, _styleButton);
                GUI.color = prev;
            }

            private bool Click(Rect rect)
            {
                if (_blockInput) return false;
                return GUI.Button(rect, GUIContent.none, _styleBlank);
            }
            #endregion

            #region 绘制原语
            private float S(float value)
            {
                return value * _scale;
            }

            private Rect R(float x, float y, float w, float h)
            {
                return new Rect(_px + x * _scale, _py + y * _scale, w * _scale, h * _scale);
            }

            private Rect L(float x, float y, float w, float h)
            {
                return new Rect(x * _scale, y * _scale, w * _scale, h * _scale);
            }

            private void DrawRect(Rect rect, Color color)
            {
                if (rect.width <= 0 || rect.height <= 0) return;
                var prev = GUI.color;
                GUI.color = color;
                GUI.DrawTexture(rect, _white);
                GUI.color = prev;
            }

            private void DrawRounded(Rect rect, Color color, float radiusPx, Color? border, float borderPx)
            {
                if (rect.width <= 0 || rect.height <= 0) return;
                var radius = Mathf.Min(radiusPx, Mathf.Min(rect.width, rect.height) * 0.5f);

                if (border.HasValue && borderPx > 0)
                {
                    DrawRoundedCore(rect, border.Value, radius);
                    var inset = borderPx;
                    DrawRoundedCore(
                        new Rect(rect.x + inset, rect.y + inset, rect.width - inset * 2, rect.height - inset * 2),
                        color, Mathf.Max(0f, radius - inset));
                }
                else
                {
                    DrawRoundedCore(rect, color, radius);
                }
            }

            private void DrawRoundedCore(Rect rect, Color color, float radius)
            {
                if (rect.width <= 0 || rect.height <= 0) return;
                var r = Mathf.Round(radius);
                if (r < 1f)
                {
                    DrawRect(rect, color);
                    return;
                }

                var corners = GetCorners((int)r, color);
                var prev = GUI.color;

                GUI.color = color;
                GUI.DrawTexture(new Rect(rect.x + r, rect.y + r, rect.width - r * 2, rect.height - r * 2), _white);
                GUI.DrawTexture(new Rect(rect.x + r, rect.y, rect.width - r * 2, r), _white);
                GUI.DrawTexture(new Rect(rect.x + r, rect.yMax - r, rect.width - r * 2, r), _white);
                GUI.DrawTexture(new Rect(rect.x, rect.y + r, r, rect.height - r * 2), _white);
                GUI.DrawTexture(new Rect(rect.xMax - r, rect.y + r, r, rect.height - r * 2), _white);
                GUI.color = prev;

                GUI.DrawTexture(new Rect(rect.x, rect.y, r, r), corners[0]);
                GUI.DrawTexture(new Rect(rect.xMax - r, rect.y, r, r), corners[1]);
                GUI.DrawTexture(new Rect(rect.x, rect.yMax - r, r, r), corners[2]);
                GUI.DrawTexture(new Rect(rect.xMax - r, rect.yMax - r, r, r), corners[3]);
            }

            private Texture2D[] GetCorners(int radius, Color color)
            {
                var key = ColorKey(color) ^ ((long)radius << 40);
                if (_cornerSets.TryGetValue(key, out var set)) return set;

                var size = Mathf.Max(4, radius * 2);
                var textures = new Texture2D[4];
                for (int corner = 0; corner < 4; corner++)
                {
                    var tex = new Texture2D(size, size, TextureFormat.RGBA32, false);
                    var pixels = new Color32[size * size];
                    for (int y = 0; y < size; y++)
                    {
                        for (int x = 0; x < size; x++)
                        {
                            float cx = corner == 1 || corner == 3 ? 0f : size;
                            float cy = corner == 2 || corner == 3 ? 0f : size;
                            float dx = x + 0.5f - cx;
                            float dy = y + 0.5f - cy;
                            float dist = Mathf.Sqrt(dx * dx + dy * dy);
                            float alpha = Mathf.Clamp01(size - dist + 0.5f);
                            var c = color;
                            c.a *= alpha;
                            pixels[y * size + x] = c;
                        }
                    }
                    tex.SetPixels32(pixels);
                    tex.Apply();
                    tex.filterMode = FilterMode.Bilinear;
                    tex.wrapMode = TextureWrapMode.Clamp;
                    tex.hideFlags = HideFlags.HideAndDontSave;
                    textures[corner] = tex;
                }

                _cornerSets[key] = textures;
                return textures;
            }

            private readonly Dictionary<long, Texture2D[]> _cornerSets = new Dictionary<long, Texture2D[]>();

            private void DrawCircle(Rect rect, Color color)
            {
                var radius = Mathf.Max(1, Mathf.RoundToInt(rect.width * 0.5f));
                var key = ColorKey(color) ^ ((long)radius << 40);
                if (!_circleCache.TryGetValue(key, out var tex) || tex == null)
                {
                    var size = Mathf.Max(4, radius * 2);
                    tex = new Texture2D(size, size, TextureFormat.RGBA32, false);
                    var pixels = new Color32[size * size];
                    var center = size * 0.5f;
                    for (int y = 0; y < size; y++)
                    {
                        for (int x = 0; x < size; x++)
                        {
                            var dx = x + 0.5f - center;
                            var dy = y + 0.5f - center;
                            var alpha = Mathf.Clamp01(center - Mathf.Sqrt(dx * dx + dy * dy) + 0.5f);
                            var c = color;
                            c.a *= alpha;
                            pixels[y * size + x] = c;
                        }
                    }
                    tex.SetPixels32(pixels);
                    tex.Apply();
                    tex.filterMode = FilterMode.Bilinear;
                    tex.wrapMode = TextureWrapMode.Clamp;
                    tex.hideFlags = HideFlags.HideAndDontSave;
                    _circleCache[key] = tex;
                }
                GUI.DrawTexture(rect, tex);
            }

            private void DrawDashedRect(Rect rect, Color color, float dash, float gap, float thickness)
            {
                var d = S(dash);
                var g = S(gap);
                var t = Mathf.Max(1f, thickness);
                var prev = GUI.color;
                GUI.color = color;

                for (float x = rect.x; x < rect.xMax; x += d + g)
                {
                    var w = Mathf.Min(d, rect.xMax - x);
                    GUI.DrawTexture(new Rect(x, rect.y, w, t), _white);
                    GUI.DrawTexture(new Rect(x, rect.yMax - t, w, t), _white);
                }
                for (float y = rect.y; y < rect.yMax; y += d + g)
                {
                    var h = Mathf.Min(d, rect.yMax - y);
                    GUI.DrawTexture(new Rect(rect.x, y, t, h), _white);
                    GUI.DrawTexture(new Rect(rect.xMax - t, y, t, h), _white);
                }
                GUI.color = prev;
            }

            private Texture2D MakeRoundedBoxTexture(int radius, Color fill, Color stroke, int strokePx)
            {
                var size = radius * 2 + 4;
                var tex = new Texture2D(size, size, TextureFormat.RGBA32, false);
                var pixels = new Color32[size * size];
                for (int y = 0; y < size; y++)
                {
                    for (int x = 0; x < size; x++)
                    {
                        var px = x + 0.5f;
                        var py = y + 0.5f;
                        var outer = RoundedSdf(px, py, size, size, radius);
                        var outerA = Mathf.Clamp01(0.5f - outer);
                        var inner = RoundedSdf(px, py, size, size, Mathf.Max(0, radius - strokePx)) + strokePx;
                        var innerA = Mathf.Clamp01(0.5f - inner);
                        var c = Blend(stroke, outerA, fill, innerA);
                        pixels[y * size + x] = c;
                    }
                }
                tex.SetPixels32(pixels);
                tex.Apply();
                tex.filterMode = FilterMode.Bilinear;
                tex.wrapMode = TextureWrapMode.Clamp;
                tex.hideFlags = HideFlags.HideAndDontSave;
                return tex;
            }

            private static Color32 Blend(Color under, float underA, Color over, float overA)
            {
                var outA = overA + underA * (1f - overA);
                if (outA <= 0.0001f) return new Color32(0, 0, 0, 0);
                var r = (over.r * overA + under.r * underA * (1f - overA)) / outA;
                var g = (over.g * overA + under.g * underA * (1f - overA)) / outA;
                var b = (over.b * overA + under.b * underA * (1f - overA)) / outA;
                return new Color(r, g, b, outA);
            }

            private static float RoundedSdf(float x, float y, float w, float h, float radius)
            {
                var qx = Mathf.Abs(x - w * 0.5f) - (w * 0.5f - radius);
                var qy = Mathf.Abs(y - h * 0.5f) - (h * 0.5f - radius);
                var ax = Mathf.Max(qx, 0f);
                var ay = Mathf.Max(qy, 0f);
                return Mathf.Sqrt(ax * ax + ay * ay) + Mathf.Min(Mathf.Max(qx, qy), 0f) - radius;
            }

            private Texture2D MakeLogoTexture()
            {
                const int size = 64;
                var tex = new Texture2D(size, size, TextureFormat.RGBA32, false);
                var pixels = new Color32[size * size];
                var center = size * 0.5f;
                var color = C(151, 206, 224, 1f);

                for (int y = 0; y < size; y++)
                {
                    for (int x = 0; x < size; x++)
                    {
                        var dx = Mathf.Abs(x + 0.5f - center);
                        var dy = Mathf.Abs(y + 0.5f - center);
                        // 尖顶六边形：max(|x|*0.866 + |y|*0.5, |y|) <= R
                        var d = Mathf.Max(dx * 0.866f + dy * 0.5f, dy);
                        var ring = Mathf.Clamp01(2.0f - Mathf.Abs(d - 25.5f));
                        var dot = Mathf.Clamp01(6f - Mathf.Sqrt(dx * dx + dy * dy));
                        var alpha = Mathf.Max(ring, dot > 0 ? 0.9f * dot : 0f);
                        var c = color;
                        c.a = Mathf.Clamp01(alpha);
                        pixels[y * size + x] = c;
                    }
                }
                tex.SetPixels32(pixels);
                tex.Apply();
                tex.filterMode = FilterMode.Bilinear;
                tex.hideFlags = HideFlags.HideAndDontSave;
                return tex;
            }

            private Texture2D MakeMagnifierTexture()
            {
                const int size = 32;
                var tex = new Texture2D(size, size, TextureFormat.RGBA32, false);
                var pixels = new Color32[size * size];
                var color = C(125, 148, 162, 1f);

                for (int y = 0; y < size; y++)
                {
                    for (int x = 0; x < size; x++)
                    {
                        var dx = x + 0.5f - 12.5f;
                        var dy = y + 0.5f - 12.5f;
                        var ring = Mathf.Clamp01(2.0f - Mathf.Abs(Mathf.Sqrt(dx * dx + dy * dy) - 8f));
                        // 手柄：从右下 45° 延伸
                        var handle = 0f;
                        if (x > 17 && y > 17)
                        {
                            var along = (x + y) * 0.7071f;
                            var across = Mathf.Abs((x - y) * 0.7071f);
                            if (along > 24f && along < 33f) handle = Mathf.Clamp01(3.2f - across);
                        }
                        var c = color;
                        c.a = Mathf.Clamp01(Mathf.Max(ring, handle));
                        pixels[y * size + x] = c;
                    }
                }
                tex.SetPixels32(pixels);
                tex.Apply();
                tex.filterMode = FilterMode.Bilinear;
                tex.hideFlags = HideFlags.HideAndDontSave;
                return tex;
            }

            private static long ColorKey(Color color)
            {
                var c = (Color32)color;
                return ((long)c.r << 24) | ((long)c.g << 16) | ((long)c.b << 8) | c.a;
            }
            #endregion

            #region 格式化
            private static Color C(int r, int g, int b, float a)
            {
                return new Color(r / 255f, g / 255f, b / 255f, a);
            }

            private static string MakeDisplayName(string fileName)
            {
                var name = Path.GetFileNameWithoutExtension(fileName ?? string.Empty);
                if (name.StartsWith("_autosave_", StringComparison.OrdinalIgnoreCase))
                {
                    if (int.TryParse(name.Substring("_autosave_".Length), out var index))
                    {
                        return "自动存档 " + (index + 1);
                    }
                    return "自动存档";
                }
                if (string.Equals(name, "_lastexit_", StringComparison.OrdinalIgnoreCase)) return "上次退出";

                var split = name.IndexOf(" - ", StringComparison.Ordinal);
                if (split > 0) return name.Substring(0, split) + " · " + name.Substring(split + 3);
                return name;
            }

            private static string FormatSize(long bytes)
            {
                if (bytes < 0) bytes = 0;
                if (bytes >= 1024L * 1024 * 1024)
                {
                    return (bytes / (1024.0 * 1024 * 1024)).ToString("0.0", CultureInfo.InvariantCulture) + " GB";
                }
                if (bytes >= 1024L * 1024)
                {
                    return (bytes / (1024.0 * 1024)).ToString("0.0", CultureInfo.InvariantCulture) + " MB";
                }
                if (bytes >= 1024L)
                {
                    return (bytes / 1024.0).ToString("0", CultureInfo.InvariantCulture) + " KB";
                }
                return bytes.ToString(CultureInfo.InvariantCulture) + " B";
            }

            private static string FormatQuota(long used, long total)
            {
                if (total >= 1024L * 1024 * 1024)
                {
                    return (used / (1024.0 * 1024 * 1024)).ToString("0.0", CultureInfo.InvariantCulture) +
                           " / " + FormatSize(total);
                }
                if (total >= 1024L * 1024)
                {
                    return (used / (1024.0 * 1024)).ToString("0.0", CultureInfo.InvariantCulture) +
                           " / " + FormatSize(total);
                }
                return FormatSize(used) + " / " + FormatSize(total);
            }

            private static string FormatTime(DateTime time)
            {
                return time.Year < 2000 ? "—" : time.ToString("MM-dd HH:mm", CultureInfo.InvariantCulture);
            }

            private static DateTime FromUnixMs(long ms)
            {
                if (ms <= 0) return DateTime.MinValue;
                try { return DateTimeOffset.FromUnixTimeMilliseconds(ms).UtcDateTime; }
                catch { return DateTime.MinValue; }
            }

            private static string FormatEta(double seconds)
            {
                if (seconds < 0) return "剩余时间计算中";
                if (seconds < 1) return "即将完成";
                if (seconds < 60) return $"剩余约 {seconds:0} 秒";
                if (seconds < 3600) return $"剩余约 {(int)(seconds / 60)} 分 {(int)(seconds % 60)} 秒";
                return $"剩余约 {(int)(seconds / 3600)} 小时 {(int)((seconds % 3600) / 60)} 分";
            }

            private static string Trim(string text, int maxChars)
            {
                if (string.IsNullOrEmpty(text)) return string.Empty;
                text = text.Replace("\r", " ").Replace("\n", " ");
                if (text.Length <= maxChars) return text;
                return text.Substring(0, maxChars - 1) + "…";
            }

            private static string ShortError(Exception ex)
            {
                if (ex == null) return "未知错误";
                var message = ex.Message ?? ex.GetType().Name;
                message = message.Replace("\r", " ").Replace("\n", " ");
                return message.Length > 60 ? message.Substring(0, 59) + "…" : message;
            }
            #endregion

            #region 数据模型
            private sealed class SaveRow
            {
                public readonly string FileName;
                public readonly string DisplayName;
                public string LocalPath;

                public bool LocalExists;
                public long LocalSize;
                public DateTime LocalMtimeUtc;

                public bool RemoteExists;
                public long RemoteSize;
                public long RemoteMtimeMs;
                public RemoteFile Remote;

                private int _kind;
                private int _status;
                private long _transferred;
                private long _total;
                private long _speed;
                private long _speedSampleBytes;
                private DateTime _speedSampleUtc;
                public string Error;

                // 刚上传成功后的短暂标记：服务端 mtime 是“提交时间”，可能比本地
                // mtime 新；仅凭时间会被误判成“云端较新”。标记在本地文件再次变化
                // 或超过一段时间后自动失效。
                private bool _syncedByUpload;
                private long _uploadedSize;
                private DateTime _uploadedLocalMtime;
                private DateTime _uploadedAtUtc;

                public CancellationTokenSource Cts;

                public SaveRow(RemoteFile remote, FileInfo local)
                {
                    var name = remote?.FileName ?? local?.Name ?? "unknown.dsv";
                    FileName = name;
                    DisplayName = MakeDisplayName(name);
                    LocalPath = local?.FullName;

                    if (remote != null)
                    {
                        Remote = remote;
                        RemoteExists = true;
                        RemoteSize = remote.FileSize;
                        RemoteMtimeMs = remote.LastModified;
                    }

                    if (local != null)
                    {
                        try
                        {
                            // 自动存档轮换时文件可能在枚举后已被删除
                            if (local.Exists)
                            {
                                LocalExists = true;
                                LocalSize = local.Length;
                                LocalMtimeUtc = local.LastWriteTimeUtc;
                                LocalPath = local.FullName;
                            }
                        }
                        catch
                        {
                            LocalExists = false;
                        }
                    }
                }

                public TransferKind Kind => (TransferKind)Volatile.Read(ref _kind);
                public RowStatus Status => (RowStatus)Volatile.Read(ref _status);
                public bool IsBusy
                {
                    get
                    {
                        var status = Status;
                        return Kind != TransferKind.None &&
                               (status == RowStatus.Queued || status == RowStatus.Running);
                    }
                }
                public long Transferred => Interlocked.Read(ref _transferred);
                public long Total => Interlocked.Read(ref _total);
                public long Speed => Interlocked.Read(ref _speed);

                public float Progress
                {
                    get
                    {
                        var total = Total;
                        if (total <= 0) return 0f;
                        return Mathf.Clamp01((float)(Transferred / (double)total));
                    }
                }

                public double EtaSeconds
                {
                    get
                    {
                        var speed = Speed;
                        if (speed <= 0) return -1;
                        var remaining = Total - Transferred;
                        if (remaining <= 0) return 0;
                        return remaining / (double)speed;
                    }
                }

                public long MaxSize => Math.Max(LocalSize, RemoteSize);

                public DateTime RecentUtc
                {
                    get
                    {
                        var remote = FromUnixMs(RemoteMtimeMs);
                        return LocalMtimeUtc > remote ? LocalMtimeUtc : remote;
                    }
                }

                public RowDirection Direction
                {
                    get
                    {
                        if (LocalExists && RemoteExists)
                        {
                            // 上传刚完成：只要云端时间与上传时间接近，就认为已一致。
                            // 不使用“当前时间”，否则 10 分钟后会莫名其妙变回“云端较新”。
                            if (_syncedByUpload && LocalSize == RemoteSize && RemoteMtimeMs > 0 &&
                                Math.Abs(RemoteMtimeMs - ToUnixMs(_uploadedAtUtc)) <= 10 * 60 * 1000)
                            {
                                return RowDirection.Synced;
                            }
                            if (LocalSize == RemoteSize && TimeClose) return RowDirection.Synced;
                            var localMs = ToUnixMs(LocalMtimeUtc);
                            return RemoteMtimeMs > 0 && RemoteMtimeMs >= localMs
                                ? RowDirection.Download
                                : RowDirection.Upload;
                        }
                        if (LocalExists) return RowDirection.Upload;
                        if (RemoteExists) return RowDirection.Download;
                        return RowDirection.None;
                    }
                }

                private bool TimeClose
                {
                    get
                    {
                        if (RemoteMtimeMs <= 0) return true;
                        return Math.Abs(RemoteMtimeMs - ToUnixMs(LocalMtimeUtc)) <= ClockToleranceMs;
                    }
                }

                public void Begin(TransferKind kind)
                {
                    Error = null;
                    Interlocked.Exchange(ref _transferred, 0);
                    Interlocked.Exchange(ref _total, kind == TransferKind.Upload ? LocalSize : RemoteSize);
                    Interlocked.Exchange(ref _speed, 0);
                    _speedSampleBytes = 0;
                    _speedSampleUtc = DateTime.UtcNow;
                    Cts?.Dispose();
                    Cts = new CancellationTokenSource();
                    Volatile.Write(ref _status, (int)RowStatus.Queued);
                    Volatile.Write(ref _kind, (int)kind);
                }

                public void MarkRunning()
                {
                    Volatile.Write(ref _status, (int)RowStatus.Running);
                    _speedSampleBytes = Transferred;
                    _speedSampleUtc = DateTime.UtcNow;
                }

                public void AddProgress(long delta)
                {
                    Interlocked.Add(ref _transferred, delta);
                }

                public void FillProgress()
                {
                    var total = Total;
                    if (total > 0) Interlocked.Exchange(ref _transferred, total);
                }

                public void UpdateSpeed()
                {
                    var now = DateTime.UtcNow;
                    var elapsed = (now - _speedSampleUtc).TotalSeconds;
                    if (elapsed < 0.4) return;

                    var bytes = Transferred;
                    var instant = (bytes - _speedSampleBytes) / Math.Max(0.001, elapsed);
                    var previous = Speed;
                    var smoothed = previous <= 0 ? instant : previous * 0.6 + instant * 0.4;
                    Interlocked.Exchange(ref _speed, (long)Math.Max(0, smoothed));
                    _speedSampleBytes = bytes;
                    _speedSampleUtc = now;
                }

                public void Complete()
                {
                    Volatile.Write(ref _status, (int)RowStatus.Idle);
                    Volatile.Write(ref _kind, (int)TransferKind.None);
                    Error = null;
                    Interlocked.Exchange(ref _speed, 0);
                }

                public void Fail(string message)
                {
                    Error = message;
                    Volatile.Write(ref _status, (int)RowStatus.Failed);
                    Volatile.Write(ref _kind, (int)TransferKind.None);
                    Interlocked.Exchange(ref _speed, 0);
                }

                public void Canceled()
                {
                    Error = null;
                    Volatile.Write(ref _status, (int)RowStatus.Idle);
                    Volatile.Write(ref _kind, (int)TransferKind.None);
                    Interlocked.Exchange(ref _speed, 0);
                }

                public void End()
                {
                    Volatile.Write(ref _kind, (int)TransferKind.None);
                    if (Volatile.Read(ref _status) == (int)RowStatus.Running)
                    {
                        Volatile.Write(ref _status, (int)RowStatus.Idle);
                    }
                    try { Cts?.Dispose(); } catch { }
                    Cts = null;
                }

                public void MarkUploaded()
                {
                    RefreshLocal();
                    if (!LocalExists) return;
                    // 上传成功即代表云端已有一份相同大小；即使随后拉取列表失败，
                    // 也先按“已一致”展示，避免玩家误点下载自己的刚上传文件。
                    RemoteExists = true;
                    RemoteSize = LocalSize;
                    RemoteMtimeMs = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
                    _uploadedSize = LocalSize;
                    _uploadedLocalMtime = LocalMtimeUtc;
                    _uploadedAtUtc = DateTime.UtcNow;
                    _syncedByUpload = true;
                }

                public void UpdateFrom(RemoteFile remote, FileInfo local)
                {
                    Remote = remote;
                    RemoteExists = remote != null;
                    RemoteSize = remote?.FileSize ?? 0;
                    RemoteMtimeMs = remote?.LastModified ?? 0;

                    LocalExists = false;
                    LocalSize = 0;
                    LocalMtimeUtc = DateTime.MinValue;
                    if (local != null)
                    {
                        try
                        {
                            if (local.Exists)
                            {
                                LocalExists = true;
                                LocalSize = local.Length;
                                LocalMtimeUtc = local.LastWriteTimeUtc;
                                LocalPath = local.FullName;
                            }
                        }
                        catch
                        {
                            LocalExists = false;
                        }
                    }

                    if (_syncedByUpload &&
                        (!LocalExists || !RemoteExists || LocalSize != _uploadedSize ||
                         LocalMtimeUtc != _uploadedLocalMtime))
                    {
                        _syncedByUpload = false;
                    }
                }

                public void ApplyRemote(RemoteFile remote)
                {
                    Remote = remote;
                    RemoteExists = remote != null;
                    RemoteSize = remote?.FileSize ?? 0;
                    RemoteMtimeMs = remote?.LastModified ?? 0;
                }

                public void RefreshLocal()
                {
                    if (string.IsNullOrEmpty(LocalPath)) return;
                    try
                    {
                        var info = new FileInfo(LocalPath);
                        if (!info.Exists)
                        {
                            LocalExists = false;
                            LocalSize = 0;
                            LocalMtimeUtc = DateTime.MinValue;
                            return;
                        }
                        LocalExists = true;
                        LocalSize = info.Length;
                        LocalMtimeUtc = info.LastWriteTimeUtc;
                    }
                    catch
                    {
                        // 文件被游戏占用时保持旧值，下次刷新会纠正
                    }
                }
            }
            #endregion
        }
    }
}
