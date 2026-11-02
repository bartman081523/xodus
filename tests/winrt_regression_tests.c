#define COBJMACROS
#include <windows.h>
#include <stdio.h>
#include <assert.h>

typedef void *HSTRING;
typedef enum { RO_INIT_SINGLETHREADED = 0, RO_INIT_MULTITHREADED = 1 } RO_INIT_TYPE;
typedef enum { AsyncStatus_Started = 0, AsyncStatus_Completed = 1, AsyncStatus_Canceled = 2, AsyncStatus_Error = 3 } AsyncStatus;

typedef struct IAsyncInfoVtbl
{
    HRESULT (WINAPI *QueryInterface)(void *this, const GUID *riid, void **ppv);
    ULONG (WINAPI *AddRef)(void *this);
    ULONG (WINAPI *Release)(void *this);
    HRESULT (WINAPI *GetIids)(void *this, ULONG *iidCount, GUID **iids);
    HRESULT (WINAPI *GetRuntimeClassName)(void *this, HSTRING *className);
    HRESULT (WINAPI *GetTrustLevel)(void *this, int *trustLevel);
    HRESULT (WINAPI *get_Id)(void *this, UINT32 *id);
    HRESULT (WINAPI *get_Status)(void *this, AsyncStatus *status);
    HRESULT (WINAPI *get_ErrorCode)(void *this, HRESULT *errorCode);
    HRESULT (WINAPI *Cancel)(void *this);
    HRESULT (WINAPI *Close)(void *this);
} IAsyncInfoVtbl;

typedef struct IAsyncOperationBoolVtbl
{
    HRESULT (WINAPI *QueryInterface)(void *this, const GUID *riid, void **ppv);
    ULONG (WINAPI *AddRef)(void *this);
    ULONG (WINAPI *Release)(void *this);
    HRESULT (WINAPI *GetIids)(void *this, ULONG *iidCount, GUID **iids);
    HRESULT (WINAPI *GetRuntimeClassName)(void *this, HSTRING *className);
    HRESULT (WINAPI *GetTrustLevel)(void *this, int *trustLevel);
    HRESULT (WINAPI *put_Completed)(void *this, void *handler);
    HRESULT (WINAPI *get_Completed)(void *this, void **handler);
    HRESULT (WINAPI *GetResults)(void *this, BOOLEAN *results);
} IAsyncOperationBoolVtbl;

typedef struct IAsyncOperationCompletedHandlerBoolVtbl
{
    HRESULT (WINAPI *QueryInterface)(void *this, const GUID *riid, void **ppv);
    ULONG (WINAPI *AddRef)(void *this);
    ULONG (WINAPI *Release)(void *this);
    HRESULT (WINAPI *Invoke)(void *this, void *asyncInfo, AsyncStatus asyncStatus);
} IAsyncOperationCompletedHandlerBoolVtbl;

static LONG test_callback_invoked = 0;
static AsyncStatus test_callback_status = (AsyncStatus)-1;

static HRESULT WINAPI test_handler_QueryInterface(void *this, const GUID *riid, void **ppv)
{
    *ppv = this;
    return S_OK;
}
static ULONG WINAPI test_handler_AddRef(void *this) { return 2; }
static ULONG WINAPI test_handler_Release(void *this) { return 1; }
static HRESULT WINAPI test_handler_Invoke(void *this, void *asyncInfo, AsyncStatus asyncStatus)
{
    test_callback_invoked = 1;
    test_callback_status = asyncStatus;
    return S_OK;
}

static IAsyncOperationCompletedHandlerBoolVtbl test_handler_vtbl = {
    test_handler_QueryInterface,
    test_handler_AddRef,
    test_handler_Release,
    test_handler_Invoke
};
static void *test_handler_instance = &test_handler_vtbl;

static LONG test_pointer_pressed_invoked = 0;
static UINT32 test_pointer_device_type = 0;

static HRESULT WINAPI test_pointer_handler_QueryInterface(void *this, const GUID *riid, void **ppv)
{
    *ppv = this;
    return S_OK;
}
static ULONG WINAPI test_pointer_handler_AddRef(void *this) { return 2; }
static ULONG WINAPI test_pointer_handler_Release(void *this) { return 1; }
static HRESULT WINAPI test_pointer_handler_Invoke(void *this, void *sender, void *args)
{
    test_pointer_pressed_invoked++;
    if (args)
    {
        void **args_vtbl = *(void ***)args;
        /* Slot 6: get_CurrentPoint */
        HRESULT (WINAPI *pGetPoint)(void *, void **) = (void *)args_vtbl[6];
        void *point = NULL;
        if (SUCCEEDED(pGetPoint(args, &point)) && point)
        {
            void **pt_vtbl = *(void ***)point;
            /* Slot 6: get_PointerDevice */
            HRESULT (WINAPI *pGetDevice)(void *, void **) = (void *)pt_vtbl[6];
            void *device = NULL;
            if (SUCCEEDED(pGetDevice(point, &device)) && device)
            {
                void **dev_vtbl = *(void ***)device;
                /* Slot 6: get_PointerDeviceType */
                HRESULT (WINAPI *pGetType)(void *, UINT32 *) = (void *)dev_vtbl[6];
                pGetType(device, &test_pointer_device_type);
            }
        }
    }
    return S_OK;
}

static struct
{
    HRESULT (WINAPI *QueryInterface)(void *this, const GUID *riid, void **ppv);
    ULONG (WINAPI *AddRef)(void *this);
    ULONG (WINAPI *Release)(void *this);
    HRESULT (WINAPI *Invoke)(void *this, void *sender, void *args);
} test_pointer_handler_vtbl = {
    test_pointer_handler_QueryInterface,
    test_pointer_handler_AddRef,
    test_pointer_handler_Release,
    test_pointer_handler_Invoke
};
static void *test_pointer_handler_instance = &test_pointer_handler_vtbl;

static const GUID IID_IConsentBridgeFactory = {0xc01f054a, 0xc57c, 0x5f26, {0x61, 0x45, 0x3c, 0x93, 0xdf, 0xec, 0x77, 0x0b}};

static HRESULT (WINAPI *pRoInitialize)(RO_INIT_TYPE);
static HRESULT (WINAPI *pWindowsCreateString)(const WCHAR *, UINT32, HSTRING *);
static HRESULT (WINAPI *pWindowsDeleteString)(HSTRING);
static HRESULT (WINAPI *pRoGetActivationFactory)(HSTRING, const GUID *, void **);
static HRESULT (WINAPI *pRoActivateInstance)(HSTRING, void **);

static int run_combase_tests(void)
{
    printf("=== [TEST 1] combase ConsentBridge stubs ===\n");
    HMODULE combase = LoadLibraryA("combase.dll");
    if (!combase) { printf("FAIL: could not load combase.dll\n"); return 1; }

    pRoInitialize = (void *)GetProcAddress(combase, "RoInitialize");
    pWindowsCreateString = (void *)GetProcAddress(combase, "WindowsCreateString");
    pWindowsDeleteString = (void *)GetProcAddress(combase, "WindowsDeleteString");
    pRoGetActivationFactory = (void *)GetProcAddress(combase, "RoGetActivationFactory");
    pRoActivateInstance = (void *)GetProcAddress(combase, "RoActivateInstance");

    if (!pRoInitialize || !pWindowsCreateString || !pRoGetActivationFactory || !pRoActivateInstance)
    {
        printf("FAIL: missing combase exports\n");
        return 1;
    }

    pRoInitialize(RO_INIT_MULTITHREADED);

    /* 1.1 Test ConsentBridgeSettings */
    HSTRING str_settings = NULL;
    void *settings = NULL;
    pWindowsCreateString(L"McgBridge.Uwp.Consent.ConsentBridgeSettings", 43, &str_settings);
    HRESULT hr = pRoActivateInstance(str_settings, &settings);
    printf("  1.1 RoActivateInstance(ConsentBridgeSettings): hr=0x%08lx, ptr=%p\n", hr, settings);
    if (FAILED(hr) || !settings) { printf("FAIL: ConsentBridgeSettings activation failed\n"); return 1; }

    /* 1.2 Test ConsentBridge Factory */
    HSTRING str_bridge = NULL;
    void *factory = NULL;
    pWindowsCreateString(L"McgBridge.Uwp.Consent.ConsentBridge", 35, &str_bridge);
    hr = pRoGetActivationFactory(str_bridge, &IID_IConsentBridgeFactory, &factory);
    printf("  1.2 RoGetActivationFactory(ConsentBridge): hr=0x%08lx, ptr=%p\n", hr, factory);
    if (FAILED(hr) || !factory) { printf("FAIL: ConsentBridge factory resolution failed\n"); return 1; }

    /* 1.3 Test IConsentBridgeFactory::CreateInstance (slot 6) */
    void **factory_vtbl = *(void ***)factory;
    HRESULT (WINAPI *pCreateInstance)(void *, void *, void **) = (void *)factory_vtbl[6];
    void *bridge = NULL;
    hr = pCreateInstance(factory, settings, &bridge);
    printf("  1.3 CreateInstance(settings=%p): hr=0x%08lx, bridge=%p\n", settings, hr, bridge);
    if (FAILED(hr) || !bridge) { printf("FAIL: CreateInstance failed\n"); return 1; }

    /* 1.4 Test IConsentBridge::CheckConsentRequired (slot 8) */
    void **bridge_vtbl = *(void ***)bridge;
    HRESULT (WINAPI *pCheckConsentRequired)(void *, void **) = (void *)bridge_vtbl[8];
    void *async_op = NULL;
    hr = pCheckConsentRequired(bridge, &async_op);
    printf("  1.4 CheckConsentRequired: hr=0x%08lx, async_op=%p\n", hr, async_op);
    if (FAILED(hr) || !async_op) { printf("FAIL: CheckConsentRequired failed\n"); return 1; }

    /* 1.5 Test IAsyncOperation<boolean>::GetResults */
    IAsyncOperationBoolVtbl *op_vtbl = *(IAsyncOperationBoolVtbl **)async_op;
    BOOLEAN result = TRUE;
    hr = op_vtbl->GetResults(async_op, &result);
    printf("  1.5 GetResults: hr=0x%08lx, result=%d (expected 0 / FALSE)\n", hr, (int)result);
    if (FAILED(hr) || result != FALSE) { printf("FAIL: GetResults did not return FALSE\n"); return 1; }

    /* 1.6 Test IAsyncOperation<boolean>::put_Completed invoking handler */
    test_callback_invoked = 0;
    hr = op_vtbl->put_Completed(async_op, &test_handler_instance);
    printf("  1.6 put_Completed: hr=0x%08lx, invoked=%ld, status=%d\n", hr, test_callback_invoked, (int)test_callback_status);
    if (FAILED(hr) || !test_callback_invoked || test_callback_status != AsyncStatus_Completed)
    {
        printf("FAIL: put_Completed did not synchronously invoke callback with Completed status\n");
        return 1;
    }

    printf("PASS: combase ConsentBridge stubs verified!\n\n");
    return 0;
}

static int run_xaml_tests(void)
{
    printf("=== [TEST 2] windows.ui.xaml Window & Dispatcher ===\n");
    HMODULE xaml = LoadLibraryA("windows.ui.xaml.dll");
    if (!xaml) { printf("FAIL: could not load windows.ui.xaml.dll\n"); return 1; }

    HMODULE combase = LoadLibraryA("combase.dll");
    HRESULT (WINAPI *pRoInitialize)(RO_INIT_TYPE) = (void *)GetProcAddress(combase, "RoInitialize");
    HRESULT (WINAPI *pWindowsCreateString)(const WCHAR *, UINT32, HSTRING *) = (void *)GetProcAddress(combase, "WindowsCreateString");
    HRESULT (WINAPI *pRoGetActivationFactory)(HSTRING, const GUID *, void **) = (void *)GetProcAddress(combase, "RoGetActivationFactory");

    pRoInitialize(RO_INIT_MULTITHREADED);

    /* Test WindowStatics */
    static const GUID IID_IWindowStatics = {0x93328409, 0x4ea1, 0x4afa, {0x83, 0xdc, 0x0c, 0x4e, 0x73, 0xe8, 0x8b, 0xb1}};
    HSTRING str_win = NULL;
    void *win_statics = NULL;
    pWindowsCreateString(L"Windows.UI.Xaml.Window", 22, &str_win);
    HRESULT hr = pRoGetActivationFactory(str_win, &IID_IWindowStatics, &win_statics);
    printf("  2.1 RoGetActivationFactory(Windows.UI.Xaml.Window): hr=0x%08lx, statics=%p\n", hr, win_statics);
    if (FAILED(hr) || !win_statics) { printf("FAIL: Window statics resolution failed\n"); return 1; }

    /* Get Current Window (slot 6) */
    void **statics_vtbl = *(void ***)win_statics;
    HRESULT (WINAPI *pGetCurrent)(void *, void **) = (void *)statics_vtbl[6];
    void *curr_window = NULL;
    hr = pGetCurrent(win_statics, &curr_window);
    printf("  2.2 get_Current: hr=0x%08lx, window=%p\n", hr, curr_window);
    if (FAILED(hr) || !curr_window) { printf("FAIL: get_Current window failed\n"); return 1; }

    /* Get Dispatcher (slot 10) */
    void **win_vtbl = *(void ***)curr_window;
    HRESULT (WINAPI *pGetDispatcher)(void *, void **) = (void *)win_vtbl[10];
    void *dispatcher = NULL;
    hr = pGetDispatcher(curr_window, &dispatcher);
    printf("  2.3 get_Dispatcher: hr=0x%08lx, dispatcher=%p\n", hr, dispatcher);
    if (FAILED(hr) || !dispatcher) { printf("FAIL: get_Dispatcher failed\n"); return 1; }

    /* ICoreDispatcher::get_HasThreadAccess (slot 6 of ICoreDispatcher) */
    void **disp_vtbl = *(void ***)dispatcher;
    HRESULT (WINAPI *pGetHasThreadAccess)(void *, BOOLEAN *) = (void *)disp_vtbl[6];
    BOOLEAN has_access = FALSE;
    hr = pGetHasThreadAccess(dispatcher, &has_access);
    printf("  2.4 get_HasThreadAccess: hr=0x%08lx, has_access=%d\n", hr, (int)has_access);
    if (FAILED(hr)) { printf("FAIL: get_HasThreadAccess failed\n"); return 1; }

    printf("PASS: windows.ui.xaml Window and Dispatcher verified!\n\n");
    return 0;
}

/* IBuffer / IBufferByteAccess mock for testing */
static const GUID my_IID_IBufferByteAccess = {0x905a0fef, 0xbc53, 0x11df, {0x8c, 0x49, 0x00, 0x1e, 0x4f, 0xc6, 0x86, 0xda}};
static const GUID my_IID_IBuffer = {0xe2f4e58e, 0xcc7d, 0x4272, {0x9c, 0x8e, 0x30, 0xc1, 0xfa, 0x49, 0x41, 0xae}};

struct test_buffer {
    void *vtbl_ibuffer;
    void *vtbl_byteaccess;
    LONG ref;
    UINT32 capacity;
    UINT32 length;
    BYTE data[4096];
};

static HRESULT WINAPI test_get_buffer(void *This, BYTE **value) {
    struct test_buffer *b = (struct test_buffer *)((char *)This - sizeof(void *));
    *value = b->data;
    return S_OK;
}
static HRESULT WINAPI test_byteaccess_QI(void *This, REFIID riid, void **ppv) {
    *ppv = This;
    return S_OK;
}
static ULONG WINAPI test_addref(void *This) { return 2; }
static ULONG WINAPI test_release(void *This) { return 1; }

static void *global_byteaccess_vtbl[4] = {
    (void *)test_byteaccess_QI,
    (void *)test_addref,
    (void *)test_release,
    (void *)test_get_buffer
};

static HRESULT WINAPI test_ibuffer_QI(void *This, REFIID riid, void **ppv) {
    struct test_buffer *b = (struct test_buffer *)This;
    if (IsEqualGUID(riid, &my_IID_IBufferByteAccess)) {
        *ppv = &b->vtbl_byteaccess;
        return S_OK;
    }
    *ppv = This;
    return S_OK;
}
static HRESULT WINAPI test_get_capacity(void *This, UINT32 *cap) {
    struct test_buffer *b = (struct test_buffer *)This;
    *cap = b->capacity;
    return S_OK;
}
static HRESULT WINAPI test_get_length(void *This, UINT32 *len) {
    struct test_buffer *b = (struct test_buffer *)This;
    *len = b->length;
    return S_OK;
}
static HRESULT WINAPI test_put_length(void *This, UINT32 len) {
    struct test_buffer *b = (struct test_buffer *)This;
    b->length = len;
    return S_OK;
}

static void *global_ibuffer_vtbl[9] = {
    (void *)test_ibuffer_QI, (void *)test_addref, (void *)test_release,
    NULL, NULL, NULL,
    (void *)test_get_capacity, (void *)test_get_length, (void *)test_put_length
};

static void init_test_buffer(struct test_buffer *b) {
    memset(b, 0, sizeof(*b));
    b->capacity = sizeof(b->data);
    b->ref = 1;
    b->vtbl_ibuffer = global_ibuffer_vtbl;
    b->vtbl_byteaccess = global_byteaccess_vtbl;
}

static int run_networking_tests(void)
{
    printf("=== [TEST 3] windows.networking Sockets & Streams ===\n");
    HMODULE net = LoadLibraryA("windows.networking.dll");
    if (!net) { printf("FAIL: could not load windows.networking.dll\n"); return 1; }

    HMODULE combase = LoadLibraryA("combase.dll");
    HRESULT (WINAPI *pRoInitialize)(RO_INIT_TYPE) = (void *)GetProcAddress(combase, "RoInitialize");
    HRESULT (WINAPI *pWindowsCreateString)(const WCHAR *, UINT32, HSTRING *) = (void *)GetProcAddress(combase, "WindowsCreateString");
    HRESULT (WINAPI *pRoActivateInstance)(HSTRING, void **) = (void *)GetProcAddress(combase, "RoActivateInstance");

    pRoInitialize(RO_INIT_MULTITHREADED);

    /* 3.1 Activate StreamSocket */
    HSTRING str_socket = NULL;
    void *socket_inst = NULL;
    pWindowsCreateString(L"Windows.Networking.Sockets.StreamSocket", 39, &str_socket);
    HRESULT hr = pRoActivateInstance(str_socket, &socket_inst);
    printf("  3.1 RoActivateInstance(StreamSocket): hr=0x%08lx, socket=%p\n", hr, socket_inst);
    if (FAILED(hr) || !socket_inst) { printf("FAIL: StreamSocket activation failed\n"); return 1; }

    /* 3.2 Query streams (InputStream slot 8, OutputStream slot 9) */
    void **sock_vtbl = *(void ***)socket_inst;
    HRESULT (WINAPI *pGetInputStream)(void *, void **) = (void *)sock_vtbl[8];
    HRESULT (WINAPI *pGetOutputStream)(void *, void **) = (void *)sock_vtbl[9];

    void *in_stream = NULL;
    void *out_stream = NULL;
    hr = pGetInputStream(socket_inst, &in_stream);
    printf("  3.2 get_InputStream: hr=0x%08lx, stream=%p\n", hr, in_stream);
    if (FAILED(hr) || !in_stream) { printf("FAIL: get_InputStream failed\n"); return 1; }

    hr = pGetOutputStream(socket_inst, &out_stream);
    printf("  3.3 get_OutputStream: hr=0x%08lx, stream=%p\n", hr, out_stream);
    if (FAILED(hr) || !out_stream) { printf("FAIL: get_OutputStream failed\n"); return 1; }

    /* 3.4 Test .archive 404 response synthesis */
    {
        struct test_buffer buf;
        init_test_buffer(&buf);

        /* Prepare HTTP GET request for a DLC .archive file */
        const char req_archive[] = "GET /dlc/worldevent_dlc.archive HTTP/1.1\r\nHost: cdn.msgamestudios.com\r\n\r\n";
        buf.length = (UINT32)strlen(req_archive);
        memcpy(buf.data, req_archive, buf.length);

        /* WriteAsync (slot 6 of IOutputStream) */
        void **out_vtbl = *(void ***)out_stream;
        HRESULT (WINAPI *pWriteAsync)(void *, void *, void **) = (void *)out_vtbl[6];
        void *async_write = NULL;
        hr = pWriteAsync(out_stream, &buf, &async_write);
        printf("  3.4 WriteAsync(.archive request): hr=0x%08lx\n", hr);
        if (FAILED(hr)) { printf("FAIL: WriteAsync failed\n"); return 1; }

        /* Read back response from InputStream (slot 6: ReadAsync(buffer, count, options, &op)) */
        struct test_buffer read_buf;
        init_test_buffer(&read_buf);

        void **in_vtbl = *(void ***)in_stream;
        HRESULT (WINAPI *pReadAsync)(void *, void *, UINT32, int, void **) = (void *)in_vtbl[6];
        void *async_read = NULL;
        hr = pReadAsync(in_stream, &read_buf, (UINT32)(sizeof(read_buf.data) - 1), 0, &async_read);
        printf("  3.5 ReadAsync: hr=0x%08lx, bytes_read=%u\n", hr, read_buf.length);
        if (FAILED(hr) || read_buf.length == 0) { printf("FAIL: ReadAsync failed to get synthesized response\n"); return 1; }
        read_buf.data[read_buf.length] = '\0';
        printf("  3.6 Received HTTP response:\n%.60s...\n", (char *)read_buf.data);
        if (strstr((char *)read_buf.data, "HTTP/1.1 404 Not Found"))
        {
            printf("PASS: .archive request correctly rejected with 404 Not Found!\n");
        }
        else
        {
            printf("FAIL: .archive request did not receive 404 Not Found!\n");
            return 1;
        }

        /* 3.7 Test secondary manifest (starclub) 200 OK synthesis */
        const char req_manifest[] = "GET /dlc/starclub_manifest.json HTTP/1.1\r\nHost: cdn.msgamestudios.com\r\n\r\n";
        init_test_buffer(&buf);
        buf.length = (UINT32)strlen(req_manifest);
        memcpy(buf.data, req_manifest, buf.length);

        hr = pWriteAsync(out_stream, &buf, &async_write);
        printf("  3.7 WriteAsync(starclub_manifest request): hr=0x%08lx\n", hr);
        if (FAILED(hr)) { printf("FAIL: WriteAsync manifest failed\n"); return 1; }

        init_test_buffer(&read_buf);
        hr = pReadAsync(in_stream, &read_buf, (UINT32)(sizeof(read_buf.data) - 1), 0, &async_read);
        printf("  3.8 ReadAsync(manifest): hr=0x%08lx, bytes_read=%u\n", hr, read_buf.length);
        if (FAILED(hr) || read_buf.length == 0) { printf("FAIL: ReadAsync manifest failed\n"); return 1; }
        read_buf.data[read_buf.length] = '\0';
        printf("  3.9 Received HTTP response:\n%.80s...\n", (char *)read_buf.data);
        if (strstr((char *)read_buf.data, "HTTP/1.1 200 OK") && strstr((char *)read_buf.data, "\"categories\": []"))
        {
            printf("PASS: starclub_manifest correctly returned 200 OK with clean empty categories!\n");
        }
        else
        {
            printf("FAIL: starclub_manifest did not receive expected 200 OK with empty categories!\n");
            return 1;
        }
    }

    printf("PASS: windows.networking StreamSocket and Streams verified!\n\n");
    return 0;
}

static const GUID IID_ISwapChainPanelFactory = {0xf38f8d7f, 0x1a48, 0x49cb, {0x86, 0xd2, 0x10, 0xea, 0xaa, 0xf6, 0xfd, 0x70}};
static const GUID IID_ISwapChainPanel        = {0xc589644f, 0xeba8, 0x427a, {0xb7, 0x5a, 0x9f, 0x1f, 0x93, 0xa1, 0x1a, 0xe9}};
static const GUID IID_ICoreInputSourceBase   = {0x9f488807, 0x4580, 0x4be8, {0xbe, 0x68, 0x92, 0xa9, 0x31, 0x17, 0x13, 0xbb}};
static const GUID IID_ICorePointerInputSource = {0xbbf1bb18, 0xe47a, 0x48eb, {0x88, 0x07, 0xf8, 0xf8, 0xd3, 0xea, 0x45, 0x51}};
static const GUID IID_ICorePointerInputSource_Alt = {0xbcf7b858, 0xe4b2, 0x48fb, {0x88, 0x07, 0xf8, 0xf8, 0xd3, 0xea, 0x45, 0x51}};

static int run_swapchain_input_tests(void)
{
    printf("=== [TEST 4] SwapChainPanel CreateCoreIndependentInputSource and ICorePointerInputSource ===\n");
    HMODULE combase = LoadLibraryA("combase.dll");
    HRESULT (WINAPI *pRoInitialize)(RO_INIT_TYPE) = (void *)GetProcAddress(combase, "RoInitialize");
    HRESULT (WINAPI *pWindowsCreateString)(const WCHAR *, UINT32, HSTRING *) = (void *)GetProcAddress(combase, "WindowsCreateString");
    HRESULT (WINAPI *pRoGetActivationFactory)(HSTRING, const GUID *, void **) = (void *)GetProcAddress(combase, "RoGetActivationFactory");

    pRoInitialize(RO_INIT_MULTITHREADED);

    HSTRING str_scp = NULL;
    void *scp_factory = NULL;
    pWindowsCreateString(L"Windows.UI.Xaml.Controls.SwapChainPanel", 39, &str_scp);
    HRESULT hr = pRoGetActivationFactory(str_scp, &IID_ISwapChainPanelFactory, &scp_factory);
    printf("  4.1 RoGetActivationFactory(SwapChainPanel): hr=0x%08lx, factory=%p\n", hr, scp_factory);
    if (FAILED(hr) || !scp_factory) { printf("FAIL: SwapChainPanel factory failed\n"); return 1; }

    /* CreateInstance on factory (slot 6: CreateInstance(IInspectable *outer, IInspectable **inner, ISwapChainPanel **instance)) */
    void **fac_vtbl = *(void ***)scp_factory;
    HRESULT (WINAPI *pCreateInstance)(void *, void *, void **, void **) = (void *)fac_vtbl[6];
    void *scp_instance = NULL;
    void *inner = NULL;
    hr = pCreateInstance(scp_factory, NULL, &inner, &scp_instance);
    printf("  4.2 ISwapChainPanelFactory::CreateInstance: hr=0x%08lx, scp=%p\n", hr, scp_instance);
    if (FAILED(hr) || !scp_instance) { printf("FAIL: CreateInstance failed\n"); return 1; }

    /* ISwapChainPanel slot 10: CreateCoreIndependentInputSource(int deviceTypes, void **ppResult) */
    void **scp_vtbl = *(void ***)scp_instance;
    HRESULT (WINAPI *pCreateCoreIndependentInputSource)(void *, int, void **) = (void *)scp_vtbl[10];
    void *input_source = NULL;
    hr = pCreateCoreIndependentInputSource(scp_instance, 7, &input_source);
    printf("  4.3 CreateCoreIndependentInputSource(7): hr=0x%08lx, input_source=%p\n", hr, input_source);
    if (FAILED(hr) || !input_source) { printf("FAIL: CreateCoreIndependentInputSource failed\n"); return 1; }

    /* Query ICoreInputSourceBase */
    void **is_vtbl = *(void ***)input_source;
    HRESULT (WINAPI *pQI)(void *, const GUID *, void **) = (void *)is_vtbl[0];
    void *input_base = NULL;
    hr = pQI(input_source, &IID_ICoreInputSourceBase, &input_base);
    printf("  4.4 QueryInterface(ICoreInputSourceBase): hr=0x%08lx, base=%p\n", hr, input_base);
    if (FAILED(hr) || !input_base) { printf("FAIL: QI(ICoreInputSourceBase) failed\n"); return 1; }

    /* ICoreInputSourceBase::get_Dispatcher (slot 6) */
    void **base_vtbl = *(void ***)input_base;
    HRESULT (WINAPI *pGetDisp)(void *, void **) = (void *)base_vtbl[6];
    void *disp = NULL;
    hr = pGetDisp(input_base, &disp);
    printf("  4.5 ICoreInputSourceBase::get_Dispatcher: hr=0x%08lx, disp=%p\n", hr, disp);
    if (FAILED(hr) || !disp) { printf("FAIL: get_Dispatcher failed\n"); return 1; }

    /* ICoreInputSourceBase::get_IsInputEnabled (slot 7) */
    HRESULT (WINAPI *pGetEnabled)(void *, BOOLEAN *) = (void *)base_vtbl[7];
    BOOLEAN enabled = FALSE;
    hr = pGetEnabled(input_base, &enabled);
    printf("  4.6 ICoreInputSourceBase::get_IsInputEnabled: hr=0x%08lx, enabled=%d\n", hr, (int)enabled);
    if (FAILED(hr) || !enabled) { printf("FAIL: get_IsInputEnabled failed\n"); return 1; }

    /* Query ICorePointerInputSource (primary GUID) */
    void *ptr_source = NULL;
    hr = pQI(input_source, &IID_ICorePointerInputSource, &ptr_source);
    printf("  4.7 QueryInterface(ICorePointerInputSource {bbf1...}): hr=0x%08lx, ptr_source=%p\n", hr, ptr_source);
    if (FAILED(hr) || !ptr_source) { printf("FAIL: QI(ICorePointerInputSource) failed\n"); return 1; }

    /* Query ICorePointerInputSource (alternate SDK GUID) */
    void *ptr_source_alt = NULL;
    hr = pQI(input_source, &IID_ICorePointerInputSource_Alt, &ptr_source_alt);
    printf("  4.8 QueryInterface(ICorePointerInputSource_Alt {bcf7...}): hr=0x%08lx, ptr_source_alt=%p\n", hr, ptr_source_alt);
    if (FAILED(hr) || !ptr_source_alt) { printf("FAIL: QI(ICorePointerInputSource_Alt) failed\n"); return 1; }

    /* Test event registrations on ICorePointerInputSource */
    void **ptr_vtbl = *(void ***)ptr_source;
    INT64 token = 0;
    /* Slot 20: add_PointerPressed */
    HRESULT (WINAPI *pAddPressed)(void *, void *, INT64 *) = (void *)ptr_vtbl[20];
    hr = pAddPressed(ptr_source, &test_handler_instance, &token);
    printf("  4.9 add_PointerPressed: hr=0x%08lx, token=%lld\n", hr, (long long)token);
    if (FAILED(hr) || token == 0) { printf("FAIL: add_PointerPressed failed\n"); return 1; }

    /* Slot 22: add_PointerReleased */
    token = 0;
    HRESULT (WINAPI *pAddReleased)(void *, void *, INT64 *) = (void *)ptr_vtbl[22];
    hr = pAddReleased(ptr_source, &test_handler_instance, &token);
    printf("  4.10 add_PointerReleased: hr=0x%08lx, token=%lld\n", hr, (long long)token);
    if (FAILED(hr) || token == 0) { printf("FAIL: add_PointerReleased failed\n"); return 1; }

    /* Slot 18: add_PointerMoved */
    token = 0;
    HRESULT (WINAPI *pAddMoved)(void *, void *, INT64 *) = (void *)ptr_vtbl[18];
    hr = pAddMoved(ptr_source, &test_handler_instance, &token);
    printf("  4.11 add_PointerMoved: hr=0x%08lx, token=%lld\n", hr, (long long)token);
    if (FAILED(hr) || token == 0) { printf("FAIL: add_PointerMoved failed\n"); return 1; }

    /* Slot 24: add_PointerWheelChanged */
    token = 0;
    HRESULT (WINAPI *pAddWheel)(void *, void *, INT64 *) = (void *)ptr_vtbl[24];
    hr = pAddWheel(ptr_source, &test_handler_instance, &token);
    printf("  4.12 add_PointerWheelChanged: hr=0x%08lx, token=%lld\n", hr, (long long)token);
    if (FAILED(hr) || token == 0) { printf("FAIL: add_PointerWheelChanged failed\n"); return 1; }

    /* Register test_pointer_handler for PointerPressed */
    INT64 pressed_token = 0;
    hr = pAddPressed(ptr_source, &test_pointer_handler_instance, &pressed_token);
    printf("  4.13 Register test handler for PointerPressed: hr=0x%08lx, token=%lld\n", hr, (long long)pressed_token);
    if (FAILED(hr) || pressed_token == 0) { printf("FAIL: could not register test pointer handler\n"); return 1; }

    /* Find XAML Window and dispatch WM_LBUTTONDOWN message */
    HWND xaml_hwnd = FindWindowW(L"Windows.UI.Xaml.Window", NULL);
    printf("  4.14 FindWindowW(Windows.UI.Xaml.Window): hwnd=%p\n", xaml_hwnd);
    if (xaml_hwnd)
    {
        SendMessageW(xaml_hwnd, WM_LBUTTONDOWN, MK_LBUTTON, MAKELPARAM(100, 100));
        SendMessageW(xaml_hwnd, WM_LBUTTONUP, 0, MAKELPARAM(100, 100));
        printf("  4.15 Pointer handler invoked: count=%ld, device_type=%u (expected 2 / Mouse)\n",
               test_pointer_pressed_invoked, test_pointer_device_type);
        if (test_pointer_pressed_invoked <= 0 || test_pointer_device_type != 2)
        {
            printf("FAIL: PointerPressed event dispatch or PointerDevice resolution failed\n");
            return 1;
        }
    }
    else
    {
        printf("  (Notice: XAML window not yet created in this test process, skipping SendMessage)\n");
    }

    printf("PASS: SwapChainPanel CreateCoreIndependentInputSource and ICorePointerInputSource verified!\n\n");
    return 0;
}

static volatile LONG test_storage_handler_invoked = 0;
static volatile int test_storage_handler_status = -1;

static HRESULT WINAPI test_storage_completed_QI(void *This, REFIID riid, void **ppv)
{
    if (!ppv) return E_POINTER;
    *ppv = This;
    return S_OK;
}
static ULONG WINAPI test_storage_completed_AddRef(void *This) { return 1; }
static ULONG WINAPI test_storage_completed_Release(void *This) { return 1; }
static HRESULT WINAPI test_storage_completed_Invoke(void *This, void *asyncInfo, int status)
{
    test_storage_handler_invoked++;
    test_storage_handler_status = status;
    return S_OK;
}
static const void *test_storage_completed_vtbl[4] = {
    (void *)test_storage_completed_QI,
    (void *)test_storage_completed_AddRef,
    (void *)test_storage_completed_Release,
    (void *)test_storage_completed_Invoke
};
static struct { const void *lpVtbl; } test_storage_completed_instance = { test_storage_completed_vtbl };

static int run_storage_tests(void)
{
    printf("=== SUITE 5: Windows.Storage.ApplicationData & Folder Async Tests ===\n");
    HSTRING app_data_str = NULL;
    HRESULT hr = pWindowsCreateString(L"Windows.Storage.ApplicationData", 31, &app_data_str);
    if (FAILED(hr)) { printf("FAIL: WindowsCreateString failed\n"); return 1; }

    static const GUID IID_IApplicationDataStatics = {0x5612147b, 0xe843, 0x45e3, {0x94, 0xd8, 0x06, 0x16, 0x9e, 0x3c, 0x8e, 0x17}};
    void *app_data_statics = NULL;
    hr = pRoGetActivationFactory(app_data_str, &IID_IApplicationDataStatics, &app_data_statics);
    pWindowsDeleteString(app_data_str);
    printf("  5.1 RoGetActivationFactory(Windows.Storage.ApplicationData): hr=0x%08lx, factory=%p\n", hr, app_data_statics);
    if (FAILED(hr) || !app_data_statics) { printf("FAIL: could not get ApplicationData factory\n"); return 1; }

    void **statics_vtbl = *(void ***)app_data_statics;
    /* Slot 6: get_Current */
    HRESULT (WINAPI *pGetCurrent)(void *, void **) = (void *)statics_vtbl[6];
    void *app_data = NULL;
    hr = pGetCurrent(app_data_statics, &app_data);
    printf("  5.2 ApplicationData.get_Current: hr=0x%08lx, app_data=%p\n", hr, app_data);
    if (FAILED(hr) || !app_data) { printf("FAIL: get_Current failed\n"); return 1; }

    void **app_data_vtbl = *(void ***)app_data;
    /* Slot 12: get_LocalFolder */
    HRESULT (WINAPI *pGetLocalFolder)(void *, void **) = (void *)app_data_vtbl[12];
    void *local_folder = NULL;
    hr = pGetLocalFolder(app_data, &local_folder);
    printf("  5.3 ApplicationData.get_LocalFolder: hr=0x%08lx, local_folder=%p\n", hr, local_folder);
    if (FAILED(hr) || !local_folder) { printf("FAIL: get_LocalFolder failed\n"); return 1; }

    void **folder_vtbl = *(void ***)local_folder;
    /* Slot 9: CreateFolderAsync(name, options, &operation) */
    HSTRING folder_name_str = NULL;
    pWindowsCreateString(L"test_async_folder", 17, &folder_name_str);

    HRESULT (WINAPI *pCreateFolderAsync)(void *, HSTRING, int, void **) = (void *)folder_vtbl[9];
    void *async_create_op = NULL;
    test_storage_handler_invoked = 0;
    test_storage_handler_status = -1;

    hr = pCreateFolderAsync(local_folder, folder_name_str, 3 /* OpenIfExists */, &async_create_op);
    printf("  5.4 CreateFolderAsync(test_async_folder, OpenIfExists): hr=0x%08lx, async_op=%p\n", hr, async_create_op);
    if (FAILED(hr) || !async_create_op) { printf("FAIL: CreateFolderAsync failed\n"); return 1; }

    /* Set completion handler on async_create_op */
    void **async_create_vtbl = *(void ***)async_create_op;
    /* Slot 6: put_Completed */
    HRESULT (WINAPI *pPutCompleted)(void *, void *) = (void *)async_create_vtbl[6];
    hr = pPutCompleted(async_create_op, &test_storage_completed_instance);
    printf("  5.5 put_Completed: hr=0x%08lx, handler_invoked=%ld, status=%d\n",
           hr, test_storage_handler_invoked, test_storage_handler_status);
    if (FAILED(hr) || test_storage_handler_invoked <= 0 || test_storage_handler_status != 1 /* Completed */)
    {
        printf("FAIL: async completed handler was not invoked properly\n");
        return 1;
    }

    /* Slot 8: GetResults */
    HRESULT (WINAPI *pGetResults)(void *, void **) = (void *)async_create_vtbl[8];
    void *created_folder = NULL;
    hr = pGetResults(async_create_op, &created_folder);
    printf("  5.6 GetResults: hr=0x%08lx, created_folder=%p\n", hr, created_folder);
    if (FAILED(hr) || !created_folder) { printf("FAIL: GetResults on created folder failed\n"); return 1; }

    /* Slot 11: GetFolderAsync(name, &operation) */
    HRESULT (WINAPI *pGetFolderAsync)(void *, HSTRING, void **) = (void *)folder_vtbl[11];
    void *async_get_op = NULL;
    hr = pGetFolderAsync(local_folder, folder_name_str, &async_get_op);
    printf("  5.7 GetFolderAsync(test_async_folder): hr=0x%08lx, async_op=%p\n", hr, async_get_op);
    if (FAILED(hr) || !async_get_op) { printf("FAIL: GetFolderAsync failed\n"); return 1; }

    void **async_get_vtbl = *(void ***)async_get_op;
    HRESULT (WINAPI *pGetFolderResults)(void *, void **) = (void *)async_get_vtbl[8];
    void *got_folder = NULL;
    hr = pGetFolderResults(async_get_op, &got_folder);
    printf("  5.8 GetResults for GetFolderAsync: hr=0x%08lx, got_folder=%p\n", hr, got_folder);
    if (FAILED(hr) || !got_folder) { printf("FAIL: GetResults for GetFolderAsync failed\n"); return 1; }

    /* Slot 12: GetItemAsync(name, &operation) */
    HRESULT (WINAPI *pGetItemAsync)(void *, HSTRING, void **) = (void *)folder_vtbl[12];
    void *async_item_op = NULL;
    hr = pGetItemAsync(local_folder, folder_name_str, &async_item_op);
    printf("  5.9 GetItemAsync(test_async_folder): hr=0x%08lx, async_op=%p\n", hr, async_item_op);
    if (FAILED(hr) || !async_item_op) { printf("FAIL: GetItemAsync failed\n"); return 1; }

    void **async_item_vtbl = *(void ***)async_item_op;
    HRESULT (WINAPI *pGetItemResults)(void *, void **) = (void *)async_item_vtbl[8];
    void *got_item = NULL;
    hr = pGetItemResults(async_item_op, &got_item);
    printf("  5.10 GetResults for GetItemAsync: hr=0x%08lx, got_item=%p\n", hr, got_item);
    if (FAILED(hr) || !got_item) { printf("FAIL: GetResults for GetItemAsync failed\n"); return 1; }

    pWindowsDeleteString(folder_name_str);
    printf("PASS: StorageFolder CreateFolderAsync, GetFolderAsync, and GetItemAsync verified!\n\n");
    return 0;
}

static int run_package3_and_fileio_tests(void)
{
    printf("=========================================\n");
    printf("[SUITE 6] Package3 & FileIO Tests\n");
    printf("=========================================\n");

    /* 6.1 Package3 test */
    HMODULE hCombase = LoadLibraryA("combase.dll");
    if (!hCombase) { printf("FAIL: combase.dll not loaded\n"); return 1; }

    HRESULT (WINAPI *pRoGetActivationFactory)(HSTRING, const GUID *, void **) =
        (void *)GetProcAddress(hCombase, "RoGetActivationFactory");
    HRESULT (WINAPI *pWindowsCreateString)(const WCHAR *, UINT32, HSTRING *) =
        (void *)GetProcAddress(hCombase, "WindowsCreateString");
    HRESULT (WINAPI *pWindowsDeleteString)(HSTRING) =
        (void *)GetProcAddress(hCombase, "WindowsDeleteString");
    const WCHAR *(WINAPI *pWindowsGetStringRawBuffer)(HSTRING, UINT32 *) =
        (void *)GetProcAddress(hCombase, "WindowsGetStringRawBuffer");

    static const GUID iid_IPackageStatics = {0x4e534bdf, 0x2960, 0x4878, {0x97, 0xa4, 0x96, 0x24, 0xde, 0xb7, 0x2f, 0x2d}};
    static const GUID iid_IPackage3 = {0x5f738b61, 0xf86a, 0x4917, {0x93, 0xd1, 0xf1, 0xee, 0x9d, 0x3b, 0x35, 0xd9}};
    static const GUID iid_IFileIOStatics = {0x887411eb, 0x7f54, 0x4732, {0xa5, 0xf0, 0x5e, 0x43, 0xe3, 0xb8, 0xc2, 0xf5}};

    HSTRING pkg_class = NULL;
    pWindowsCreateString(L"Windows.ApplicationModel.Package", 32, &pkg_class);
    void *pkg_factory = NULL;
    HRESULT hr = pRoGetActivationFactory(pkg_class, &iid_IPackageStatics, &pkg_factory);
    pWindowsDeleteString(pkg_class);
    printf("  6.1 Package factory: hr=0x%08lx, factory=%p\n", hr, pkg_factory);
    if (FAILED(hr) || !pkg_factory) { printf("FAIL: Package factory failed\n"); return 1; }

    void **pkg_factory_vtbl = *(void ***)pkg_factory;
    /* Slot 6: get_Current */
    HRESULT (WINAPI *pGetCurrent)(void *, void **) = (void *)pkg_factory_vtbl[6];
    void *current_pkg = NULL;
    hr = pGetCurrent(pkg_factory, &current_pkg);
    printf("  6.2 Package::get_Current: hr=0x%08lx, pkg=%p\n", hr, current_pkg);
    if (FAILED(hr) || !current_pkg) { printf("FAIL: Package::get_Current failed\n"); return 1; }

    void **current_pkg_vtbl = *(void ***)current_pkg;
    HRESULT (WINAPI *pQI)(void *, const GUID *, void **) = (void *)current_pkg_vtbl[0];
    void *pkg3 = NULL;
    hr = pQI(current_pkg, &iid_IPackage3, &pkg3);
    printf("  6.3 Package::QI(IPackage3): hr=0x%08lx, pkg3=%p\n", hr, pkg3);
    if (FAILED(hr) || !pkg3) { printf("FAIL: Package::QI(IPackage3) failed\n"); return 1; }

    void **pkg3_vtbl = *(void ***)pkg3;
    /* Slot 7: get_InstalledDate */
    typedef struct { INT64 UniversalTime; } DateTime;
    HRESULT (WINAPI *pGetInstalledDate)(void *, DateTime *) = (void *)pkg3_vtbl[7];
    DateTime dt = {0};
    hr = pGetInstalledDate(pkg3, &dt);
    printf("  6.4 IPackage3::get_InstalledDate: hr=0x%08lx, UniversalTime=%lld\n", hr, (long long)dt.UniversalTime);
    if (FAILED(hr) || dt.UniversalTime == 0) { printf("FAIL: get_InstalledDate failed\n"); return 1; }

    /* 6.2 FileIO test */
    HSTRING file_io_class = NULL;
    pWindowsCreateString(L"Windows.Storage.FileIO", 22, &file_io_class);
    void *file_io_statics = NULL;
    hr = pRoGetActivationFactory(file_io_class, &iid_IFileIOStatics, &file_io_statics);
    pWindowsDeleteString(file_io_class);
    printf("  6.5 RoGetActivationFactory(Windows.Storage.FileIO): hr=0x%08lx, statics=%p\n", hr, file_io_statics);
    if (FAILED(hr) || !file_io_statics) { printf("FAIL: RoGetActivationFactory(FileIO) failed\n"); return 1; }

    /* Create test file using LocalFolder */
    HSTRING app_data_class = NULL;
    pWindowsCreateString(L"Windows.Storage.ApplicationData", 31, &app_data_class);
    static const GUID iid_IApplicationDataStatics = {0x5612147b, 0xe843, 0x45e3, {0x94, 0xd8, 0x06, 0x16, 0x9e, 0x3c, 0x8e, 0x17}};
    void *app_data_factory = NULL;
    hr = pRoGetActivationFactory(app_data_class, &iid_IApplicationDataStatics, &app_data_factory);
    pWindowsDeleteString(app_data_class);
    if (FAILED(hr) || !app_data_factory) { printf("FAIL: ApplicationData factory failed\n"); return 1; }

    void **app_data_factory_vtbl = *(void ***)app_data_factory;
    HRESULT (WINAPI *pGetCurrentAppData)(void *, void **) = (void *)app_data_factory_vtbl[6];
    void *current_app_data = NULL;
    pGetCurrentAppData(app_data_factory, &current_app_data);

    void **app_data_vtbl = *(void ***)current_app_data;
    /* Slot 12: get_LocalFolder */
    HRESULT (WINAPI *pGetLocalFolder)(void *, void **) = (void *)app_data_vtbl[12];
    void *local_folder = NULL;
    hr = pGetLocalFolder(current_app_data, &local_folder);
    printf("  6.5.1 get_LocalFolder: hr=0x%08lx, folder=%p\n", hr, local_folder);
    if (FAILED(hr) || !local_folder) { printf("FAIL: get_LocalFolder failed\n"); return 1; }

    void **folder_vtbl = *(void ***)local_folder;
    /* Slot 7: CreateFileAsync(name, options, &operation) */
    HSTRING test_file_name = NULL;
    pWindowsCreateString(L"fileio_test.txt", 15, &test_file_name);
    HRESULT (WINAPI *pCreateFileAsync)(void *, HSTRING, int, void **) = (void *)folder_vtbl[7];
    void *async_create_file = NULL;
    hr = pCreateFileAsync(local_folder, test_file_name, 1 /* ReplaceExisting */, &async_create_file);
    pWindowsDeleteString(test_file_name);
    printf("  6.6 CreateFileAsync(fileio_test.txt): hr=0x%08lx, op=%p\n", hr, async_create_file);
    if (FAILED(hr) || !async_create_file) { printf("FAIL: CreateFileAsync failed\n"); return 1; }

    void **async_create_file_vtbl = *(void ***)async_create_file;
    HRESULT (WINAPI *pGetFileResults)(void *, void **) = (void *)async_create_file_vtbl[8];
    void *storage_file = NULL;
    hr = pGetFileResults(async_create_file, &storage_file);
    printf("  6.7 StorageFile: hr=0x%08lx, file=%p\n", hr, storage_file);
    if (FAILED(hr) || !storage_file) { printf("FAIL: GetResults for CreateFileAsync failed\n"); return 1; }

    /* Test WriteTextAsync: slot 8 of IFileIOStatics */
    void **file_io_vtbl = *(void ***)file_io_statics;
    HRESULT (WINAPI *pWriteTextAsync)(void *, void *, HSTRING, void **) = (void *)file_io_vtbl[8];
    HSTRING text_to_write = NULL;
    pWindowsCreateString(L"Hello Xodus WinRT FileIO!", 25, &text_to_write);
    void *write_op = NULL;
    hr = pWriteTextAsync(file_io_statics, storage_file, text_to_write, &write_op);
    pWindowsDeleteString(text_to_write);
    printf("  6.8 FileIO::WriteTextAsync: hr=0x%08lx, op=%p\n", hr, write_op);
    if (FAILED(hr) || !write_op) { printf("FAIL: WriteTextAsync failed\n"); return 1; }

    void **write_op_vtbl = *(void ***)write_op;
    HRESULT (WINAPI *pWriteResults)(void *) = (void *)write_op_vtbl[8];
    hr = pWriteResults(write_op);
    printf("  6.9 WriteTextAsync GetResults: hr=0x%08lx\n", hr);
    if (FAILED(hr)) { printf("FAIL: WriteTextAsync results failed\n"); return 1; }

    /* Test ReadTextAsync: slot 6 of IFileIOStatics */
    HRESULT (WINAPI *pReadTextAsync)(void *, void *, void **) = (void *)file_io_vtbl[6];
    void *read_op = NULL;
    hr = pReadTextAsync(file_io_statics, storage_file, &read_op);
    printf("  6.10 FileIO::ReadTextAsync: hr=0x%08lx, op=%p\n", hr, read_op);
    if (FAILED(hr) || !read_op) { printf("FAIL: ReadTextAsync failed\n"); return 1; }

    void **read_op_vtbl = *(void ***)read_op;
    HRESULT (WINAPI *pReadResults)(void *, HSTRING *) = (void *)read_op_vtbl[8];
    HSTRING read_result = NULL;
    hr = pReadResults(read_op, &read_result);
    const WCHAR *read_raw = pWindowsGetStringRawBuffer(read_result, NULL);
    printf("  6.11 ReadTextAsync GetResults: hr=0x%08lx, content='%ls'\n", hr, read_raw ? read_raw : L"(null)");
    if (FAILED(hr) || !read_raw || wcscmp(read_raw, L"Hello Xodus WinRT FileIO!") != 0)
    {
        printf("FAIL: ReadTextAsync returned unexpected content\n");
        return 1;
    }
    pWindowsDeleteString(read_result);

    /* Test AppendTextAsync: slot 10 of IFileIOStatics */
    HRESULT (WINAPI *pAppendTextAsync)(void *, void *, HSTRING, void **) = (void *)file_io_vtbl[10];
    HSTRING append_text = NULL;
    pWindowsCreateString(L" Appended text.", 15, &append_text);
    void *append_op = NULL;
    hr = pAppendTextAsync(file_io_statics, storage_file, append_text, &append_op);
    pWindowsDeleteString(append_text);
    printf("  6.12 FileIO::AppendTextAsync: hr=0x%08lx, op=%p\n", hr, append_op);
    if (FAILED(hr) || !append_op) { printf("FAIL: AppendTextAsync failed\n"); return 1; }

    void **append_op_vtbl = *(void ***)append_op;
    HRESULT (WINAPI *pAppendResults)(void *) = (void *)append_op_vtbl[8];
    hr = pAppendResults(append_op);
    printf("  6.13 AppendTextAsync GetResults: hr=0x%08lx\n", hr);
    if (FAILED(hr)) { printf("FAIL: AppendTextAsync results failed\n"); return 1; }

    /* Verify file content after append */
    hr = pReadTextAsync(file_io_statics, storage_file, &read_op);
    read_op_vtbl = *(void ***)read_op;
    pReadResults = (void *)read_op_vtbl[8];
    hr = pReadResults(read_op, &read_result);
    read_raw = pWindowsGetStringRawBuffer(read_result, NULL);
    printf("  6.14 Post-append ReadTextAsync: hr=0x%08lx, content='%ls'\n", hr, read_raw ? read_raw : L"(null)");
    if (FAILED(hr) || !read_raw || wcscmp(read_raw, L"Hello Xodus WinRT FileIO! Appended text.") != 0)
    {
        printf("FAIL: Appended content mismatch\n");
        return 1;
    }
    pWindowsDeleteString(read_result);

    /* Test DateTimeFormatter */
    static const GUID iid_IDateTimeFormatterFactory = {0xec8d8a53, 0x1a2e, 0x412d, {0x88, 0x15, 0x3b, 0x74, 0x5f, 0xb1, 0xa2, 0xa0}};
    HSTRING dtf_class = NULL;
    pWindowsCreateString(L"Windows.Globalization.DateTimeFormatting.DateTimeFormatter", 58, &dtf_class);
    void *dtf_factory = NULL;
    hr = pRoGetActivationFactory(dtf_class, &iid_IDateTimeFormatterFactory, &dtf_factory);
    pWindowsDeleteString(dtf_class);
    printf("  6.15 RoGetActivationFactory(DateTimeFormatter): hr=0x%08lx, factory=%p\n", hr, dtf_factory);
    if (FAILED(hr) || !dtf_factory) { printf("FAIL: RoGetActivationFactory(DateTimeFormatter) failed\n"); return 1; }

    void **dtf_factory_vtbl = *(void ***)dtf_factory;
    /* Slot 6: CreateDateTimeFormatter */
    HRESULT (WINAPI *pCreateDateTimeFormatter)(void *, HSTRING, void **) = (void *)dtf_factory_vtbl[6];
    HSTRING dtf_template = NULL;
    pWindowsCreateString(L"{month.integer}/{day.integer}/{year.full}", 41, &dtf_template);
    void *dtf_instance = NULL;
    hr = pCreateDateTimeFormatter(dtf_factory, dtf_template, &dtf_instance);
    pWindowsDeleteString(dtf_template);
    printf("  6.16 CreateDateTimeFormatter: hr=0x%08lx, formatter=%p\n", hr, dtf_instance);
    if (FAILED(hr) || !dtf_instance) { printf("FAIL: CreateDateTimeFormatter failed\n"); return 1; }

    void **dtf_vtbl = *(void ***)dtf_instance;
    /* Slot 14: Format(INT64 datetime, HSTRING *result) */
    HRESULT (WINAPI *pFormat)(void *, INT64, HSTRING *) = (void *)dtf_vtbl[14];
    HSTRING formatted_date = NULL;
    hr = pFormat(dtf_instance, dt.UniversalTime, &formatted_date);
    const WCHAR *date_str = pWindowsGetStringRawBuffer(formatted_date, NULL);
    printf("  6.17 Format(UniversalTime=%lld): hr=0x%08lx, formatted='%ls'\n", (long long)dt.UniversalTime, hr, date_str ? date_str : L"(null)");
    if (FAILED(hr) || !date_str || !wcschr(date_str, L'/'))
    {
        printf("FAIL: Format failed\n");
        return 1;
    }
    pWindowsDeleteString(formatted_date);

    printf("PASS: Package3, FileIO, & DateTimeFormatter verified!\n\n");
    return 0;
}

int main(void)
{
    printf("Starting WinRT Regression Test Suite...\n\n");
    int ret = 0;
    ret |= run_combase_tests();
    ret |= run_xaml_tests();
    ret |= run_networking_tests();
    ret |= run_swapchain_input_tests();
    ret |= run_storage_tests();
    ret |= run_package3_and_fileio_tests();
    if (ret == 0)
    {
        printf("=========================================\n");
        printf("ALL REGRESSION TESTS PASSED SUCCESSFULLY!\n");
        printf("=========================================\n");
    }
    else
    {
        printf("=========================================\n");
        printf("SOME REGRESSION TESTS FAILED!\n");
        printf("=========================================\n");
    }
    return ret;
}
