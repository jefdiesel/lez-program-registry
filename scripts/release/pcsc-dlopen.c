/*
 * pcsc-dlopen.c: a stand-in for libpcsclite, linked statically into the Linux release builds of
 * lezreg (.github/workflows/release.yml).
 *
 * Why: lezreg links the LEZ wallet, which links Keycard support through the `pcsc` crate, which
 * links libpcsclite. Linked the usual way, the binary will not even start on a machine without
 * libpcsclite.so.1 (most servers, containers and many desktops), although lezreg itself never
 * talks to a smart card. Debian and Ubuntu ship no static libpcsclite.
 *
 * What: every PC/SC call opens the system's libpcsclite.so.1 with dlopen on first use and
 * forwards to it, so the wallet's Keycard commands work exactly as before where the library is
 * installed. Where it is not, every call returns SCARD_E_NO_SERVICE ("the smart card resource
 * manager is not running"), which the pcsc crate reports as an ordinary error.
 *
 * The prototypes come from the system's <winscard.h> (libpcsclite-dev), so the compiler
 * checks each signature against the real library's. The three protocol control blocks are the
 * same constants pcsc-lite's own winscard_clnt.c defines.
 */
#include <dlfcn.h>
#include <pthread.h>
#include <stddef.h>

#include <winscard.h> /* from libpcsclite-dev; cflags from pkg-config */

const SCARD_IO_REQUEST g_rgSCardT0Pci = {SCARD_PROTOCOL_T0, sizeof(SCARD_IO_REQUEST)};
const SCARD_IO_REQUEST g_rgSCardT1Pci = {SCARD_PROTOCOL_T1, sizeof(SCARD_IO_REQUEST)};
const SCARD_IO_REQUEST g_rgSCardRawPci = {SCARD_PROTOCOL_RAW, sizeof(SCARD_IO_REQUEST)};

static pthread_once_t once = PTHREAD_ONCE_INIT;
static void *lib;

static void open_lib(void) {
    lib = dlopen("libpcsclite.so.1", RTLD_NOW | RTLD_LOCAL);
}

static void *sym(const char *name) {
    pthread_once(&once, open_lib);
    return lib ? dlsym(lib, name) : NULL;
}

#define FORWARD(name, params, args)                       \
    LONG name params {                                    \
        LONG(*f) params = (LONG(*) params)sym(#name);     \
        return f ? f args : (LONG)SCARD_E_NO_SERVICE;     \
    }

FORWARD(SCardEstablishContext,
        (DWORD dwScope, LPCVOID pvReserved1, LPCVOID pvReserved2, LPSCARDCONTEXT phContext),
        (dwScope, pvReserved1, pvReserved2, phContext))
FORWARD(SCardReleaseContext, (SCARDCONTEXT hContext), (hContext))
FORWARD(SCardIsValidContext, (SCARDCONTEXT hContext), (hContext))
FORWARD(SCardConnect,
        (SCARDCONTEXT hContext, LPCSTR szReader, DWORD dwShareMode, DWORD dwPreferredProtocols,
         LPSCARDHANDLE phCard, LPDWORD pdwActiveProtocol),
        (hContext, szReader, dwShareMode, dwPreferredProtocols, phCard, pdwActiveProtocol))
FORWARD(SCardReconnect,
        (SCARDHANDLE hCard, DWORD dwShareMode, DWORD dwPreferredProtocols, DWORD dwInitialization,
         LPDWORD pdwActiveProtocol),
        (hCard, dwShareMode, dwPreferredProtocols, dwInitialization, pdwActiveProtocol))
FORWARD(SCardDisconnect, (SCARDHANDLE hCard, DWORD dwDisposition), (hCard, dwDisposition))
FORWARD(SCardBeginTransaction, (SCARDHANDLE hCard), (hCard))
FORWARD(SCardEndTransaction, (SCARDHANDLE hCard, DWORD dwDisposition), (hCard, dwDisposition))
FORWARD(SCardStatus,
        (SCARDHANDLE hCard, LPSTR szReaderName, LPDWORD pcchReaderLen, LPDWORD pdwState,
         LPDWORD pdwProtocol, LPBYTE pbAtr, LPDWORD pcbAtrLen),
        (hCard, szReaderName, pcchReaderLen, pdwState, pdwProtocol, pbAtr, pcbAtrLen))
FORWARD(SCardGetStatusChange,
        (SCARDCONTEXT hContext, DWORD dwTimeout, SCARD_READERSTATE *rgReaderStates, DWORD cReaders),
        (hContext, dwTimeout, rgReaderStates, cReaders))
FORWARD(SCardControl,
        (SCARDHANDLE hCard, DWORD dwControlCode, LPCVOID pbSendBuffer, DWORD cbSendLength,
         LPVOID pbRecvBuffer, DWORD cbRecvLength, LPDWORD lpBytesReturned),
        (hCard, dwControlCode, pbSendBuffer, cbSendLength, pbRecvBuffer, cbRecvLength,
         lpBytesReturned))
FORWARD(SCardTransmit,
        (SCARDHANDLE hCard, const SCARD_IO_REQUEST *pioSendPci, LPCBYTE pbSendBuffer,
         DWORD cbSendLength, SCARD_IO_REQUEST *pioRecvPci, LPBYTE pbRecvBuffer,
         LPDWORD pcbRecvLength),
        (hCard, pioSendPci, pbSendBuffer, cbSendLength, pioRecvPci, pbRecvBuffer, pcbRecvLength))
FORWARD(SCardListReaderGroups, (SCARDCONTEXT hContext, LPSTR mszGroups, LPDWORD pcchGroups),
        (hContext, mszGroups, pcchGroups))
FORWARD(SCardListReaders,
        (SCARDCONTEXT hContext, LPCSTR mszGroups, LPSTR mszReaders, LPDWORD pcchReaders),
        (hContext, mszGroups, mszReaders, pcchReaders))
FORWARD(SCardFreeMemory, (SCARDCONTEXT hContext, LPCVOID pvMem), (hContext, pvMem))
FORWARD(SCardCancel, (SCARDCONTEXT hContext), (hContext))
FORWARD(SCardGetAttrib, (SCARDHANDLE hCard, DWORD dwAttrId, LPBYTE pbAttr, LPDWORD pcbAttrLen),
        (hCard, dwAttrId, pbAttr, pcbAttrLen))
FORWARD(SCardSetAttrib, (SCARDHANDLE hCard, DWORD dwAttrId, LPCBYTE pbAttr, DWORD cbAttrLen),
        (hCard, dwAttrId, pbAttr, cbAttrLen))
