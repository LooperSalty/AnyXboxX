"""Task queue waiters through the public GDK interface slots."""
import ctypes

from test_runtime_cpp import (CALLBACK, MANUAL, S_OK, TASK_QUEUE_GUID, WORK_PORT,
                              TaskQueueApi, anyxboxx_only)


@anyxboxx_only
def test_task_queue_query_interface(runtime_dir):
    api = TaskQueueApi(runtime_dir)
    ptr = ctypes.c_void_p
    query = api.method(0, ctypes.c_uint32, ptr, ctypes.POINTER(ptr))
    out = ptr()
    for iid in (bytes.fromhex("0000000000000000c000000000000046"), TASK_QUEUE_GUID):
        guid = (ctypes.c_ubyte * 16).from_buffer_copy(iid)
        assert query(guid, ctypes.byref(out)) == S_OK
        assert out.value == api.object.value
    unknown = (ctypes.c_ubyte * 16).from_buffer_copy(bytes(16))
    assert query(unknown, ctypes.byref(out)) == 0x80004002 and out.value is None
    assert query(unknown, None) == 0x80004003


@anyxboxx_only
def test_waiter_dispatch_and_unregister(runtime_dir):
    api = TaskQueueApi(runtime_dir)
    ptr = ctypes.c_void_p
    create = api.method(12, ctypes.c_uint32, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ptr))
    register = api.method(20, ctypes.c_uint32, ptr, ctypes.c_int, ptr, ptr, CALLBACK,
                          ctypes.POINTER(ctypes.c_uint64))
    unregister = api.method(21, None, ptr, ctypes.c_uint64)
    dispatch = api.method(16, ctypes.c_bool, ptr, ctypes.c_int, ctypes.c_uint32)
    close = api.method(17, None, ptr)
    kernel = ctypes.windll.kernel32
    kernel.CreateEventW.argtypes = (ptr, ctypes.c_bool, ctypes.c_bool, ctypes.c_wchar_p)
    kernel.CreateEventW.restype = ptr
    kernel.SetEvent.argtypes = (ptr,)
    kernel.CloseHandle.argtypes = (ptr,)
    queue = ptr()
    assert create(MANUAL, MANUAL, ctypes.byref(queue)) == S_OK
    event = kernel.CreateEventW(None, False, False, None)
    assert event
    calls = []
    callback = CALLBACK(lambda context, canceled: calls.append((context, canceled)))
    token = ctypes.c_uint64()
    try:
        assert register(queue, WORK_PORT, event, 42, callback, ctypes.byref(token)) == S_OK
        assert token.value
        for expected in (1, 2):
            assert kernel.SetEvent(event)
            assert dispatch(queue, WORK_PORT, 2000)
            assert calls == [(42, False)] * expected
        unregister(queue, token)
        assert kernel.SetEvent(event)
        assert not dispatch(queue, WORK_PORT, 100)
    finally:
        close(queue)
        kernel.CloseHandle(event)
