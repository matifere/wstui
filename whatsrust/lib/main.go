package main

/*
#include <stdlib.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>

typedef const char* JID;

typedef struct {
	bool found;
	const char* first_name;
	const char* full_name;
	const char* push_name;
	const char* business_name;
} Contact;

typedef struct {
	JID jid;
	const char* name;
} ContactEntry;

typedef struct {
	bool found;
	int64_t muted_until;
	bool pinned;
	bool archived;
} ChatSettings;

typedef struct {
	ContactEntry* entries;
	uint32_t size;
} GetContactsResult;

typedef struct {
	char* id;
	JID chat;
	JID sender;
	int64_t timestamp;
	bool isFromMe;
	char* quoteID;
	uint16_t readBy;
} MessageInfo;

typedef struct {
	char* text;
} TextMessage;

typedef struct {
	uint8_t kind;
	char* path;
	char* fileID;
	char* caption;
} FileMessage;

typedef struct {
	MessageInfo info;
	uint8_t messageType;
	void* message;
} Message;

typedef struct {
	uint8_t kind;
	JID id;
	char* const* messageIDs;
	size_t size;
} ReceiptEvent;

typedef struct {
	char* callID;
	JID caller;
	bool isVideo;
	int64_t timestamp;
} CallOfferEvent;

typedef struct {
	char* callID;
	char* reason;
} CallTerminateEvent;

typedef struct {
	uint8_t kind;
	void* data;
} Event;

typedef void (*EventCallback)(const Event*, void*);
typedef struct {
	EventCallback callback;
	void* user_data;
} EventHandler;
static void callEventCallback(EventHandler hdl, const Event* event) {
	hdl.callback(event, hdl.user_data);
}

typedef void (*QrCallback)(const char*, void*);
static void callQrCallback(QrCallback cb, const char* code, void* user_data) {
	cb(code, user_data);
}

typedef void (*MessageHandlerCallback)(const Message*, bool, void*);
typedef struct {
	MessageHandlerCallback callback;
	void* user_data;
} MessageHandler;
static void callMessageHandler(MessageHandler hdl, bool isSync, const Message* data) {
    hdl.callback(data, isSync, hdl.user_data);
}

typedef void (*HistorySyncCallback)(uint32_t, void*);
typedef struct {
	HistorySyncCallback callback;
	void* user_data;
} HistorySyncHandler;
static void callHistorySync(HistorySyncHandler hdl, uint32_t percent) {
	hdl.callback(percent, hdl.user_data);
}

typedef void (*LogHandlerCallback)(const char*, uint8_t, void*);
typedef struct {
	LogHandlerCallback callback;
	void* user_data;
} LogHandler;
static void callLogInfo(LogHandler hdl, const char* msg, uint8_t level) {
	hdl.callback(msg, level, hdl.user_data);
}
*/
import "C"
import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"io"
	"math"
	"mime"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"
	"unsafe"

	"google.golang.org/protobuf/proto"

	_ "github.com/mattn/go-sqlite3"
	whatsmeow "github.com/polymorfa/hypermeow"
	"github.com/polymorfa/hypermeow/appstate"
	"github.com/polymorfa/hypermeow/proto/waE2E"
	"github.com/polymorfa/hypermeow/proto/waWeb"
	"github.com/polymorfa/hypermeow/store/sqlstore"
	"github.com/polymorfa/hypermeow/types"
	"github.com/polymorfa/hypermeow/types/events"
	waLog "github.com/polymorfa/hypermeow/util/log"
	"github.com/purpshell/meowcaller"
	"github.com/rs/zerolog"
)

var client *whatsmeow.Client
var callerClient *meowcaller.Client
var qrChan <-chan whatsmeow.QRChannelItem

var (
	activeCallMu sync.Mutex
	activeCall   *meowcaller.Call
	videoCmd     *exec.Cmd
	videoStdin   io.WriteCloser
	audioCmd     *exec.Cmd
	audioStdin   io.WriteCloser
	micCmd       *exec.Cmd
	camCmd       *exec.Cmd
	echoModuleID string
)

func unloadWstuiEchoModules() {
	if _, err := exec.LookPath("pactl"); err != nil {
		return
	}
	out, err := exec.Command("pactl", "list", "modules", "short").Output()
	if err != nil {
		return
	}
	for _, line := range strings.Split(string(out), "\n") {
		if strings.Contains(line, "wstui-echo") {
			fields := strings.Fields(line)
			if len(fields) > 0 {
				_ = exec.Command("pactl", "unload-module", fields[0]).Run()
			}
		}
	}
}

func cleanupCallMedia() {
	activeCallMu.Lock()
	defer activeCallMu.Unlock()

	if echoModuleID != "" {
		_ = exec.Command("pactl", "unload-module", echoModuleID).Run()
		echoModuleID = ""
	}
	unloadWstuiEchoModules()

	if videoStdin != nil {
		_ = videoStdin.Close()
		videoStdin = nil
	}
	if videoCmd != nil && videoCmd.Process != nil {
		_ = videoCmd.Process.Kill()
		videoCmd = nil
	}
	if audioStdin != nil {
		_ = audioStdin.Close()
		audioStdin = nil
	}
	if audioCmd != nil && audioCmd.Process != nil {
		_ = audioCmd.Process.Kill()
		audioCmd = nil
	}
	if micCmd != nil && micCmd.Process != nil {
		_ = micCmd.Process.Kill()
		micCmd = nil
	}
	if camCmd != nil && camCmd.Process != nil {
		_ = camCmd.Process.Kill()
		camCmd = nil
	}
	activeCall = nil
}

var logHandler C.LogHandler
var messageHandler C.MessageHandler
var eventHandler C.EventHandler

func LOG_LEVEL(level int, msg string, args ...any) {
	cmsg := C.CString(fmt.Sprintf(msg, args...))
	defer C.free(unsafe.Pointer(cmsg))
	C.callLogInfo(logHandler, cmsg, C.uint8_t(level))
}

func LOG_ERROR(msg string, args ...any) {
	LOG_LEVEL(0, msg, args...)
}
func LOG_WARN(msg string, args ...any) {
	LOG_LEVEL(1, msg, args...)
}
func LOG_INFO(msg string, args ...any) {
	LOG_LEVEL(2, msg, args...)
}
func LOG_DEBUG(msg string, args ...any) {
	LOG_LEVEL(4, msg, args...)
}

// Logger
type WrLogger struct{}

func (l *WrLogger) Errorf(msg string, args ...any) {
	LOG_ERROR(msg, args...)
}
func (l *WrLogger) Warnf(msg string, args ...any) {
	LOG_WARN(msg, args...)
}
func (l *WrLogger) Infof(msg string, args ...any) {
	LOG_INFO(msg, args...)
}
func (l *WrLogger) Debugf(msg string, args ...any) {
	LOG_DEBUG(msg, args...)
}

func (l *WrLogger) Sub(module string) waLog.Logger {
	return &WrLogger{}
}

// GetSelfId returns the current user's JID string for comparison (e.g. broadcast sender).
func GetSelfId(client *whatsmeow.Client) string {
	if client == nil || client.Store == nil || client.Store.ID == nil {
		return ""
	}
	return StrFromJid(*client.Store.ID)
}

// GetChatId returns the normalized chat id (conversation key): LID→PN, broadcast→per-sender, status as-is.
func GetChatId(client *whatsmeow.Client, chatJid *types.JID, senderJid *types.JID) string {
	if chatJid == nil {
		LOG_WARN("chatJid is nil")
		return ""
	}
	if chatJid.Server == types.BroadcastServer && chatJid.User == "status" {
		return StrFromJid(*chatJid)
	}
	if chatJid.Server == types.BroadcastServer && chatJid.User != "status" {
		if senderJid != nil {
			userId := GetUserId(client, nil, senderJid)
			if userId == GetSelfId(client) {
				return StrFromJid(*chatJid)
			}
			return userId
		}
	}
	if chatJid.Server == types.HiddenUserServer {
		ctx := context.Background()
		if pChatJid, _ := client.Store.LIDs.GetPNForLID(ctx, *chatJid); !pChatJid.IsEmpty() {
			return StrFromJid(pChatJid)
		}
	}
	return StrFromJid(*chatJid)
}

// GetUserId returns the normalized user/sender id: LID→PN when known; in groups use sender as-is (like nchat).
func GetUserId(client *whatsmeow.Client, chatJid *types.JID, userJid *types.JID) string {
	if userJid == nil {
		LOG_WARN("userJid is nil")
		return ""
	}
	if chatJid != nil && chatJid.Server == types.GroupServer {
		return StrFromJid(*userJid)
	}
	if userJid.Server == types.HiddenUserServer {
		ctx := context.Background()
		if pUserJid, _ := client.Store.LIDs.GetPNForLID(ctx, *userJid); !pUserJid.IsEmpty() {
			return StrFromJid(pUserJid)
		}
	}
	return StrFromJid(*userJid)
}

// Convert Jid to string without any mapping, use with care!
func StrFromJid(jid types.JID) string {
	return jid.User + "@" + jid.Server
}

// Convert Go JID to C JID
func jidToC(jid types.JID) C.JID {
	return C.CString(jid.ToNonAD().String())
	// return C.CString(jid.User + "@" + jid.Server)
}

// Convert C JID to Go JID
func cToJid(cjid C.JID) types.JID {
	jid, err := types.ParseJID(C.GoString(cjid))
	if err != nil {
		panic(err)
	}
	return jid
}

// contactDisplayName returns the display name for a contact (same order as Rust get_contact_name).
func contactDisplayName(c types.ContactInfo) string {
	if c.FullName != "" {
		return c.FullName
	}
	if c.FirstName != "" {
		return c.FirstName
	}
	if c.PushName != "" {
		return "~ " + c.PushName
	}
	if c.BusinessName != "" {
		return "+ " + c.BusinessName
	}
	return ""
}

//export C_SetLogHandler
func C_SetLogHandler(callback C.LogHandlerCallback, data unsafe.Pointer) {
	logHandler = C.LogHandler{
		callback:  callback,
		user_data: data,
	}
}

//export C_SetEventHandler
func C_SetEventHandler(callback C.EventCallback, data unsafe.Pointer) {
	eventHandler = C.EventHandler{
		callback:  callback,
		user_data: data,
	}
}

//export C_SetMessageHandler
func C_SetMessageHandler(callback C.MessageHandlerCallback, data unsafe.Pointer) {
	messageHandler = C.MessageHandler{
		callback:  callback,
		user_data: data,
	}
}

//export C_NewClient
func C_NewClient(dbPath *C.char) {
	goPath := C.GoString(dbPath)
	dbLog := &WrLogger{}
	container, err := sqlstore.New(context.Background(), "sqlite3", "file:"+goPath+"?_foreign_keys=on", dbLog)
	if err != nil {
		panic(err)
	}
	deviceStore, _ := container.GetFirstDevice(context.Background())
	clientLog := &WrLogger{}
	client = whatsmeow.NewClient(deviceStore, clientLog)

	voipLogFile, err := os.OpenFile("/tmp/wstui_voip.log", os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0644)
	var zLog zerolog.Logger
	if err == nil {
		zLog = zerolog.New(voipLogFile).With().Timestamp().Logger()
	} else {
		zLog = zerolog.Nop()
	}
	callerClient = meowcaller.NewClient(client, meowcaller.WithLogger(zLog))
	callerClient.OnIncomingCall(func(call *meowcaller.Call) {
		LOG_INFO("meowcaller OnIncomingCall: %s from %s", call.ID(), call.Peer().String())
		activeCallMu.Lock()
		activeCall = call
		activeCallMu.Unlock()

		call.OnEnd(func(reason string) {
			LOG_INFO("Call %s ended: %s", call.ID(), reason)
			cleanupCallMedia()
		})
	})
}

func ParseWebMessageInfo(selfJid types.JID, chatJid types.JID, webMsg *waWeb.WebMessageInfo) *types.MessageInfo {
	info := types.MessageInfo{
		MessageSource: types.MessageSource{
			Chat:     chatJid,
			IsFromMe: webMsg.GetKey().GetFromMe(),
			IsGroup:  chatJid.Server == types.GroupServer,
		},
		ID:        webMsg.GetKey().GetID(),
		PushName:  webMsg.GetPushName(),
		Timestamp: time.Unix(int64(webMsg.GetMessageTimestamp()), 0),
	}
	if info.IsFromMe {
		info.Sender = selfJid.ToNonAD()
	} else if webMsg.GetParticipant() != "" {
		info.Sender, _ = types.ParseJID(webMsg.GetParticipant())
	} else if webMsg.GetKey().GetParticipant() != "" {
		info.Sender, _ = types.ParseJID(webMsg.GetKey().GetParticipant())
	} else {
		info.Sender = chatJid
	}
	if info.Sender.IsEmpty() {
		return nil
	}
	return &info
}

func SliceIndex(list []string, value string, defaultValue int) int {
	index := slices.Index(list, value)
	if index == -1 {
		index = defaultValue
	}
	return index
}

func ExtensionByType(mimeType string, defaultExt string) string {
	ext := defaultExt
	exts, extErr := mime.ExtensionsByType(mimeType)
	if extErr == nil && len(exts) > 0 {
		// prefer common extensions over less common (.jpe, etc) returned by mime library
		preferredExts := []string{".jpg", ".jpeg"}
		sort.Slice(exts, func(i, j int) bool {
			return SliceIndex(preferredExts, exts[i], math.MaxInt32) < SliceIndex(preferredExts, exts[j], math.MaxInt32)
		})
		ext = exts[0]
	}

	return ext
}

const (
	EventTypeSyncProgress = iota
	EventTypeAppStateSyncComplete
	EventTypeReceipt
	EventTypeCallOffer
	EventTypeCallTerminate
)

const (
	MessageTypeText = iota
	MessageTypeFile
)

const (
	FileTypeImage = iota
	FileTypeVideo
	FileTypeAudio
	FileTypeDocument
	FileTypeSticker
)

func ContentToWaE2EMessage(messageType C.uint8_t, messageContent unsafe.Pointer, contextInfo *waE2E.ContextInfo) *waE2E.Message {
	switch messageType {
	case C.uint8_t(MessageTypeText):
		textMsg := (*C.TextMessage)(messageContent)
		text := C.GoString(textMsg.text)
		return &waE2E.Message{
			ExtendedTextMessage: &waE2E.ExtendedTextMessage{
				Text:        &text,
				ContextInfo: contextInfo,
			},
		}

	case C.uint8_t(MessageTypeFile):
		fileMsg := (*C.FileMessage)(messageContent)
		kind := uint8(fileMsg.kind)

		filePath := C.GoString(fileMsg.path)
		data, err := os.ReadFile(filePath)
		if err != nil {
			panic(fmt.Sprintf("read file %s err %#v", filePath, err))
		}
		mimetype := mime.TypeByExtension(filepath.Ext(filePath))

		switch kind {
		case FileTypeImage:
			uploaded, upErr := client.Upload(context.Background(), data, whatsmeow.MediaImage)
			if upErr != nil {
				panic(fmt.Sprintf("upload error %#v", upErr))
			}
			return &waE2E.Message{
				ImageMessage: &waE2E.ImageMessage{
					Caption:       proto.String(C.GoString(fileMsg.caption)),
					URL:           proto.String(uploaded.URL),
					DirectPath:    proto.String(uploaded.DirectPath),
					MediaKey:      uploaded.MediaKey,
					Mimetype:      proto.String(mimetype),
					FileEncSHA256: uploaded.FileEncSHA256,
					FileSHA256:    uploaded.FileSHA256,
					FileLength:    proto.Uint64(uint64(len(data))),
					ContextInfo:   contextInfo,
				},
			}
		case FileTypeSticker:
			uploaded, upErr := client.Upload(context.Background(), data, whatsmeow.MediaImage)
			if upErr != nil {
				panic(fmt.Sprintf("upload sticker error %#v", upErr))
			}
			return &waE2E.Message{
				StickerMessage: &waE2E.StickerMessage{
					URL:           proto.String(uploaded.URL),
					DirectPath:    proto.String(uploaded.DirectPath),
					MediaKey:      uploaded.MediaKey,
					Mimetype:      proto.String("image/webp"),
					FileEncSHA256: uploaded.FileEncSHA256,
					FileSHA256:    uploaded.FileSHA256,
					FileLength:    proto.Uint64(uint64(len(data))),
					ContextInfo:   contextInfo,
				},
			}
		case FileTypeDocument:
			uploaded, upErr := client.Upload(context.Background(), data, whatsmeow.MediaDocument)
			if upErr != nil {
				panic(fmt.Sprintf("upload error %#v", upErr))
			}
			fileName := filepath.Base(filePath)
			return &waE2E.Message{
				DocumentMessage: &waE2E.DocumentMessage{
					Caption:       proto.String(C.GoString(fileMsg.caption)),
					URL:           proto.String(uploaded.URL),
					DirectPath:    proto.String(uploaded.DirectPath),
					MediaKey:      uploaded.MediaKey,
					Mimetype:      proto.String(mimetype),
					FileEncSHA256: uploaded.FileEncSHA256,
					FileSHA256:    uploaded.FileSHA256,
					FileLength:    proto.Uint64(uint64(len(data))),
					FileName:      proto.String(fileName),
					ContextInfo:   contextInfo,
				},
			}
		default:
			panic(fmt.Sprintf("Unsupported file type: %v", kind))
		}

	default:
		panic(fmt.Sprintf("Unsupported message type: %d", messageType))
	}
}

func HandleMessage(info types.MessageInfo, msg *waE2E.Message, isSync bool) {
	// Normalize chat and sender ids (LID→PN, broadcast→per-sender) so Rust sees canonical ids.
	if normalizedChat := GetChatId(client, &info.Chat, &info.Sender); normalizedChat != "" {
		if jid, err := types.ParseJID(normalizedChat); err == nil {
			info.Chat = jid
		}
	}
	if normalizedSender := GetUserId(client, &info.Chat, &info.Sender); normalizedSender != "" {
		if jid, err := types.ParseJID(normalizedSender); err == nil {
			info.Sender = jid
		}
	}

	chat := info.Chat
	sender := info.Sender
	timestamp := info.Timestamp.Unix()

	cinfo := C.MessageInfo{
		id:        C.CString(info.ID),
		chat:      jidToC(chat),
		sender:    jidToC(sender),
		timestamp: C.int64_t(timestamp),
		isFromMe:  C.bool(info.IsFromMe),
		quoteID:   nil,
		readBy:    C.uint16_t(0),
	}

	if msg.Conversation != nil {
		ctext := C.CString(msg.GetConversation())
		defer C.free(unsafe.Pointer(ctext))

		content := (*C.TextMessage)(C.malloc(C.sizeof_TextMessage))
		content.text = ctext
		defer C.free(unsafe.Pointer(content))

		message := C.Message{
			info:        cinfo,
			messageType: C.uint8_t(MessageTypeText),
			message:     unsafe.Pointer(content),
		}

		C.callMessageHandler(messageHandler, C.bool(isSync), &message)
	}
	if msg.ExtendedTextMessage != nil {
		ext_msg := msg.GetExtendedTextMessage()

		text := ext_msg.GetText()
		ctext := C.CString(text)
		defer C.free(unsafe.Pointer(ctext))

		context_info := ext_msg.GetContextInfo()
		if context_info != nil {
			id := context_info.GetStanzaID()
			// LOG_ERROR("asdfasdf %s", co)
			if id != "" {
				cinfo.quoteID = C.CString(id)
			}
		}

		content := (*C.TextMessage)(C.malloc(C.sizeof_TextMessage))
		content.text = ctext
		defer C.free(unsafe.Pointer(content))

		message := C.Message{
			info:        cinfo,
			messageType: C.uint8_t(MessageTypeText),
			message:     unsafe.Pointer(content),
		}
		C.callMessageHandler(messageHandler, C.bool(isSync), &message)
	}
	if msg.ImageMessage != nil {
		img := msg.GetImageMessage()
		if img == nil {
			LOG_ERROR("ImageMessage is nil")
			return
		}

		ext := ExtensionByType(img.GetMimetype(), ".jpg")
		caption := img.GetCaption()

		context_info := img.GetContextInfo()
		if context_info != nil {
			id := context_info.GetStanzaID()
			if id != "" {
				cinfo.quoteID = C.CString(id)
			}
		}

		filePath := fmt.Sprintf("imgs/%s%s", info.ID, ext)

		fileId := DownloadableMessageToFileId(client, img, filePath)
		cfileId := C.CString(fileId)
		defer C.free(unsafe.Pointer(cfileId))

		cpath := C.CString(filePath)
		defer C.free(unsafe.Pointer(cpath))

		// set caption or nil
		ccaption := C.CString(caption)
		if caption == "" {
			ccaption = nil
		}
		defer C.free(unsafe.Pointer(ccaption))

		content := (*C.FileMessage)(C.malloc(C.sizeof_FileMessage))
		content.kind = C.uint8_t(FileTypeImage)
		content.path = cpath
		content.fileID = cfileId
		content.caption = ccaption
		defer C.free(unsafe.Pointer(content))

		message := C.Message{
			info:        cinfo,
			messageType: C.uint8_t(MessageTypeFile),
			message:     unsafe.Pointer(content),
		}
		C.callMessageHandler(messageHandler, C.bool(isSync), &message)
	}
	if msg.VideoMessage != nil {
		vid := msg.GetVideoMessage()
		if vid == nil {
			LOG_ERROR("VideoMessage is nil")
			return
		}

		ext := ExtensionByType(vid.GetMimetype(), ".mp4")
		caption := vid.GetCaption()

		context_info := vid.GetContextInfo()
		if context_info != nil {
			id := context_info.GetStanzaID()
			if id != "" {
				cinfo.quoteID = C.CString(id)
			}
		}

		filePath := fmt.Sprintf("videos/%s%s", info.ID, ext)
		fileId := DownloadableMessageToFileId(client, vid, filePath)
		cfileId := C.CString(fileId)
		defer C.free(unsafe.Pointer(cfileId))

		cpath := C.CString(filePath)
		defer C.free(unsafe.Pointer(cpath))

		ccaption := C.CString(caption)
		if caption == "" {
			ccaption = nil
		}
		defer C.free(unsafe.Pointer(ccaption))

		content := (*C.FileMessage)(C.malloc(C.sizeof_FileMessage))
		content.kind = C.uint8_t(FileTypeVideo)
		content.path = cpath
		content.fileID = cfileId
		content.caption = ccaption
		defer C.free(unsafe.Pointer(content))
		message := C.Message{
			info:        cinfo,
			messageType: C.uint8_t(MessageTypeFile),
			message:     unsafe.Pointer(content),
		}
		C.callMessageHandler(messageHandler, C.bool(isSync), &message)
	}
	if msg.AudioMessage != nil {
		audio := msg.GetAudioMessage()
		if audio == nil {
			LOG_ERROR("AudioMessage is nil")
			return
		}

		ext := ExtensionByType(audio.GetMimetype(), ".ogg")

		context_info := audio.GetContextInfo()
		if context_info != nil {
			id := context_info.GetStanzaID()
			if id != "" {
				cinfo.quoteID = C.CString(id)
			}
		}

		filePath := fmt.Sprintf("audios/%s%s", info.ID, ext)
		fileId := DownloadableMessageToFileId(client, audio, filePath)
		cfileId := C.CString(fileId)
		defer C.free(unsafe.Pointer(cfileId))

		cpath := C.CString(filePath)
		defer C.free(unsafe.Pointer(cpath))

		content := (*C.FileMessage)(C.malloc(C.sizeof_FileMessage))
		content.kind = C.uint8_t(FileTypeAudio)
		content.path = cpath
		content.fileID = cfileId
		content.caption = nil
		defer C.free(unsafe.Pointer(content))

		message := C.Message{
			info:        cinfo,
			messageType: C.uint8_t(MessageTypeFile),
			message:     unsafe.Pointer(content),
		}
		C.callMessageHandler(messageHandler, C.bool(isSync), &message)
	}
	if msg.DocumentMessage != nil {
		doc := msg.GetDocumentMessage()
		if doc == nil {
			LOG_ERROR("DocumentMessage is nil")
			return
		}

		caption := doc.GetCaption()

		context_info := doc.GetContextInfo()
		if context_info != nil {
			id := context_info.GetStanzaID()
			if id != "" {
				cinfo.quoteID = C.CString(id)
			}
		}

		filePath := fmt.Sprintf("docs/%s-%s", info.ID, *doc.FileName)
		fileId := DownloadableMessageToFileId(client, doc, filePath)
		cfileId := C.CString(fileId)
		defer C.free(unsafe.Pointer(cfileId))

		cpath := C.CString(filePath)
		defer C.free(unsafe.Pointer(cpath))

		ccaption := C.CString(caption)
		if caption == "" {
			ccaption = nil
		}
		defer C.free(unsafe.Pointer(ccaption))

		content := (*C.FileMessage)(C.malloc(C.sizeof_FileMessage))
		content.kind = C.uint8_t(FileTypeDocument)
		content.path = cpath
		content.fileID = cfileId
		content.caption = ccaption
		defer C.free(unsafe.Pointer(content))

		message := C.Message{
			info:        cinfo,
			messageType: C.uint8_t(MessageTypeFile),
			message:     unsafe.Pointer(content),
		}
		C.callMessageHandler(messageHandler, C.bool(isSync), &message)
	}
	if msg.StickerMessage != nil {
		sticker := msg.GetStickerMessage()
		if sticker == nil {
			LOG_ERROR("StickerMessage is nil")
			return
		}

		ext := ExtensionByType(sticker.GetMimetype(), ".webp")

		context_info := sticker.GetContextInfo()
		if context_info != nil {
			id := context_info.GetStanzaID()
			if id != "" {
				cinfo.quoteID = C.CString(id)
			}
		}

		sha := sticker.GetFileSHA256()
		var fileName string
		if len(sha) > 0 {
			fileName = hex.EncodeToString(sha)
		} else {
			fileName = info.ID
		}
		filePath := fmt.Sprintf("stickers/%s%s", fileName, ext)
		fileId := DownloadableMessageToFileId(client, sticker, filePath)
		cfileId := C.CString(fileId)
		defer C.free(unsafe.Pointer(cfileId))

		cpath := C.CString(filePath)
		defer C.free(unsafe.Pointer(cpath))

		content := (*C.FileMessage)(C.malloc(C.sizeof_FileMessage))
		content.kind = C.uint8_t(FileTypeSticker)
		content.path = cpath
		content.fileID = cfileId
		content.caption = nil
		defer C.free(unsafe.Pointer(content))

		message := C.Message{
			info:        cinfo,
			messageType: C.uint8_t(MessageTypeFile),
			message:     unsafe.Pointer(content),
		}
		C.callMessageHandler(messageHandler, C.bool(isSync), &message)
	}
}

//export C_DownloadFile
func C_DownloadFile(fileId *C.char, basePath *C.char) C.uint8_t {
	goFileId := C.GoString(fileId)
	goBasePath := C.GoString(basePath)
	status := DownloadFromFileId(client, goFileId, goBasePath)
	return C.uint8_t(status)
}

//export C_AnswerCall
func C_AnswerCall(callID *C.char) C.bool {
	activeCallMu.Lock()
	call := activeCall
	activeCallMu.Unlock()

	if call == nil {
		LOG_ERROR("No active call to answer")
		return C.bool(false)
	}

	targetID := ""
	if callID != nil {
		targetID = C.GoString(callID)
	}
	if targetID != "" && call.ID() != targetID {
		LOG_WARN("Active call ID %s doesn't match requested %s", call.ID(), targetID)
	}

	// Launch ffplay for H.264 video stream
	vCmd := exec.Command("ffplay",
		"-f", "h264",
		"-probesize", "65536",
		"-analyzeduration", "100000",
		"-fflags", "nobuffer",
		"-flags", "low_delay",
		"-framedrop",
		"-window_title", fmt.Sprintf("WhatsApp Call - %s", call.Peer().String()),
		"-i", "pipe:0",
	)
	vLog, _ := os.OpenFile("/tmp/wstui_ffplay.log", os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0644)
	if vLog != nil {
		vCmd.Stderr = vLog
		vCmd.Stdout = vLog
	}
	vIn, err := vCmd.StdinPipe()
	if err == nil && vCmd.Start() == nil {
		LOG_INFO("ffplay started with PID %d", vCmd.Process.Pid)
		activeCallMu.Lock()
		videoCmd = vCmd
		videoStdin = vIn
		activeCallMu.Unlock()

		go func() {
			_ = vCmd.Wait()
			LOG_INFO("ffplay process exited")
			activeCallMu.Lock()
			if videoCmd == vCmd {
				videoCmd = nil
				videoStdin = nil
			}
			activeCallMu.Unlock()
		}()

		var frameCount uint64
		call.ReceiveVideo(meowcaller.VideoSinkFunc(func(au []byte) {
			frameCount++
			if frameCount == 1 || frameCount%60 == 0 {
				LOG_INFO("Writing video frame #%d (%d bytes) to ffplay", frameCount, len(au))
			}
			activeCallMu.Lock()
			pipe := videoStdin
			activeCallMu.Unlock()
			if pipe != nil {
				_, _ = pipe.Write(au)
			}
		}))
	} else {
		LOG_ERROR("Failed to start ffplay for video: %v", err)
	}

	// Try loading WebRTC acoustic echo cancellation module in PulseAudio/PipeWire
	sinkName := "@DEFAULT_SINK@"
	sourceName := "@DEFAULT_SOURCE@"
	if _, err := exec.LookPath("pactl"); err == nil {
		unloadWstuiEchoModules()
		out, err := exec.Command("pactl", "load-module", "module-echo-cancel",
			"aec_method=webrtc",
			"source_name=wstui-echo-source",
			"sink_name=wstui-echo-sink").Output()
		if err == nil {
			modID := strings.TrimSpace(string(out))
			activeCallMu.Lock()
			echoModuleID = modID
			activeCallMu.Unlock()
			sinkName = "wstui-echo-sink"
			sourceName = "wstui-echo-source"
			LOG_INFO("Loaded echo-cancel module ID %s (sink: %s, source: %s)", modID, sinkName, sourceName)
		} else {
			LOG_WARN("Could not load module-echo-cancel: %v", err)
		}
	}

	// Launch audio playback (paplay -> pw-play -> ffplay)
	var aCmd *exec.Cmd
	if _, err := exec.LookPath("paplay"); err == nil {
		aCmd = exec.Command("paplay", "-d", sinkName, "--raw", "--rate=16000", "--channels=1", "--format=s16le")
	} else if _, err := exec.LookPath("pw-play"); err == nil {
		aCmd = exec.Command("pw-play", "--rate", "16000", "--channels", "1", "--format", "s16", "-")
	} else {
		aCmd = exec.Command("ffplay", "-f", "s16le", "-ar", "16000", "-ac", "1", "-nodisp", "-autoexit", "-i", "pipe:0")
	}
	aLog, _ := os.OpenFile("/tmp/wstui_audio.log", os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0644)
	if aLog != nil {
		aCmd.Stderr = aLog
		aCmd.Stdout = aLog
	}

	aIn, aErr := aCmd.StdinPipe()
	if aErr == nil && aCmd.Start() == nil {
		LOG_INFO("Audio player (%s) started with PID %d", aCmd.Path, aCmd.Process.Pid)
		activeCallMu.Lock()
		audioCmd = aCmd
		audioStdin = aIn
		activeCallMu.Unlock()

		go func() {
			_ = aCmd.Wait()
			LOG_INFO("Audio player process exited")
			activeCallMu.Lock()
			if audioCmd == aCmd {
				audioCmd = nil
				audioStdin = nil
			}
			activeCallMu.Unlock()
		}()

		var audioCount uint64
		call.Receive(meowcaller.SinkFunc(func(pcm []float32) {
			audioCount++
			if audioCount == 1 || audioCount%100 == 0 {
				LOG_DEBUG("Writing audio frame #%d (%d samples) to audio player", audioCount, len(pcm))
			}
			activeCallMu.Lock()
			pipe := audioStdin
			activeCallMu.Unlock()
			if pipe != nil {
				buf := make([]byte, len(pcm)*2)
				for i, sample := range pcm {
					if sample > 1.0 {
						sample = 1.0
					} else if sample < -1.0 {
						sample = -1.0
					}
					val := int16(sample * 32767.0)
					binary.LittleEndian.PutUint16(buf[i*2:], uint16(val))
				}
				_, _ = pipe.Write(buf)
			}
		}))
	} else {
		LOG_ERROR("Failed to start audio player: %v", aErr)
	}

	// Launch microphone capture (parec -> WhatsApp)
	if _, err := exec.LookPath("parec"); err == nil {
		mCmd := exec.Command("parec", "-d", sourceName, "--latency-msec=20", "--process-time-msec=20", "--raw", "--rate=16000", "--channels=1", "--format=s16le")
		mLog, _ := os.OpenFile("/tmp/wstui_mic.log", os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0644)
		if mLog != nil {
			mCmd.Stderr = mLog
		}
		micOut, mErr := mCmd.StdoutPipe()
		if mErr == nil && mCmd.Start() == nil {
			LOG_INFO("Microphone capture (parec) started with PID %d", mCmd.Process.Pid)
			activeCallMu.Lock()
			micCmd = mCmd
			activeCallMu.Unlock()

			go func() {
				_ = mCmd.Wait()
				LOG_INFO("Microphone capture exited")
				activeCallMu.Lock()
				if micCmd == mCmd {
					micCmd = nil
				}
				activeCallMu.Unlock()
			}()

			call.Play(meowcaller.PCMStream(micOut))
		} else {
			LOG_ERROR("Failed to start microphone capture: %v", mErr)
		}
	}

	// Launch webcam capture (ffmpeg -> WhatsApp)
	if _, err := os.Stat("/dev/video0"); err == nil {
		cCmd := exec.Command("ffmpeg",
			"-f", "v4l2",
			"-input_format", "yuyv422",
			"-video_size", "640x480",
			"-framerate", "30",
			"-i", "/dev/video0",
			"-pix_fmt", "yuv420p",
			"-c:v", "libx264",
			"-profile:v", "baseline",
			"-level", "3.0",
			"-preset", "ultrafast",
			"-tune", "zerolatency",
			"-b:v", "400k",
			"-g", "15",
			"-x264opts", "aud=1:repeat-headers=1",
			"-f", "h264",
			"pipe:1",
		)
		cLog, _ := os.OpenFile("/tmp/wstui_cam.log", os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0644)
		if cLog != nil {
			cCmd.Stderr = cLog
		}
		camOut, cErr := cCmd.StdoutPipe()
		if cErr == nil && cCmd.Start() == nil {
			LOG_INFO("Webcam capture (ffmpeg) started with PID %d", cCmd.Process.Pid)
			activeCallMu.Lock()
			camCmd = cCmd
			activeCallMu.Unlock()

			go func() {
				_ = cCmd.Wait()
				LOG_INFO("Webcam capture exited")
				activeCallMu.Lock()
				if camCmd == cCmd {
					camCmd = nil
				}
				activeCallMu.Unlock()
			}()

			go streamWebcamToCall(call, camOut)
		} else {
			LOG_WARN("Could not start webcam capture: %v", cErr)
		}
	}

	if err := call.Answer(); err != nil {
		LOG_ERROR("call.Answer error: %v", err)
		cleanupCallMedia()
		return C.bool(false)
	}

	LOG_INFO("call.Answer() succeeded for %s", call.ID())
	return C.bool(true)
}

func streamWebcamToCall(call *meowcaller.Call, r io.Reader) {
	audPattern := []byte{0x00, 0x00, 0x00, 0x01, 0x09}
	audPattern3 := []byte{0x00, 0x00, 0x01, 0x09}

	buf := make([]byte, 65536)
	var accumulated []byte
	var sentFrames uint64

	for {
		n, err := r.Read(buf)
		if n > 0 {
			accumulated = append(accumulated, buf[:n]...)

			for {
				if sentFrames == 0 {
					firstIdx := bytes.Index(accumulated, audPattern)
					if firstIdx < 0 {
						firstIdx = bytes.Index(accumulated, audPattern3)
					}
					if firstIdx > 0 {
						accumulated = accumulated[firstIdx:]
					} else if firstIdx < 0 {
						if len(accumulated) > 131072 {
							accumulated = nil
						}
						break
					}
				}

				idx := -1
				if len(accumulated) > 5 {
					if i := bytes.Index(accumulated[4:], audPattern); i >= 0 {
						idx = i + 4
					} else if i := bytes.Index(accumulated[3:], audPattern3); i >= 0 {
						idx = i + 3
					}
				}

				if idx < 0 {
					break
				}

				au := make([]byte, idx)
				copy(au, accumulated[:idx])
				accumulated = accumulated[idx:]

				if len(au) > 0 {
					sentFrames++
					if sentFrames == 1 || sentFrames%60 == 0 {
						LOG_INFO("Sent outbound camera frame #%d (%d bytes)", sentFrames, len(au))
					}
					if err := call.SendVideo(au); err != nil {
						LOG_WARN("SendVideo error: %v", err)
					}
				}
			}
		}
		if err != nil {
			LOG_INFO("Webcam stream reader ended: %v", err)
			break
		}
	}
}

//export C_HangupCall
func C_HangupCall() C.bool {
	activeCallMu.Lock()
	call := activeCall
	activeCallMu.Unlock()

	if call != nil {
		_ = call.Hangup()
		cleanupCallMedia()
		return C.bool(true)
	}
	return C.bool(false)
}

//export C_RejectCall
func C_RejectCall(callFrom *C.char, callID *C.char) C.bool {
	activeCallMu.Lock()
	call := activeCall
	activeCallMu.Unlock()

	if call != nil {
		_ = call.Reject()
		cleanupCallMedia()
	}

	if client != nil && callFrom != nil && callID != nil {
		fromJid, err := types.ParseJID(C.GoString(callFrom))
		if err == nil {
			_ = client.RejectCall(context.Background(), fromJid, C.GoString(callID))
		}
	}
	return C.bool(true)
}

func AddEventHandlers() {
	client.AddEventHandler(func(rawEvt any) {
		switch evt := rawEvt.(type) {
		case *events.MarkChatAsRead:
			LOG_DEBUG("MarkChatAsRead %v", evt.JID)

		case *events.AppStateSyncComplete:
			LOG_INFO("AppStateSyncComplete %v", evt)
			if evt.Name == appstate.WAPatchRegular {
				LOG_INFO("AppStateSyncComplete (WAPatchRegular) %v", evt)

				cevent := C.Event{
					kind: C.uint8_t(EventTypeAppStateSyncComplete),
					data: nil,
				}
				C.callEventCallback(eventHandler, &cevent)
			}

		case *events.CallOffer:
			callID := C.CString(evt.CallID)
			defer C.free(unsafe.Pointer(callID))
			callerJID := jidToC(evt.CallCreator)
			defer C.free(unsafe.Pointer(callerJID))

			isVideo := false
			if evt.Data != nil {
				if evt.Data.GetChildByTag("video").Tag == "video" {
					isVideo = true
				}
			}

			cCallOffer := (*C.CallOfferEvent)(C.malloc(C.sizeof_CallOfferEvent))
			cCallOffer.callID = callID
			cCallOffer.caller = callerJID
			cCallOffer.isVideo = C.bool(isVideo)
			cCallOffer.timestamp = C.int64_t(evt.Timestamp.Unix())

			cevent := C.Event{
				kind: C.uint8_t(EventTypeCallOffer),
				data: unsafe.Pointer(cCallOffer),
			}
			C.callEventCallback(eventHandler, &cevent)
			C.free(unsafe.Pointer(cCallOffer))

		case *events.CallTerminate:
			cleanupCallMedia()
			callID := C.CString(evt.CallID)
			defer C.free(unsafe.Pointer(callID))
			reason := C.CString(evt.Reason)
			defer C.free(unsafe.Pointer(reason))

			cCallTerm := (*C.CallTerminateEvent)(C.malloc(C.sizeof_CallTerminateEvent))
			cCallTerm.callID = callID
			cCallTerm.reason = reason

			cevent := C.Event{
				kind: C.uint8_t(EventTypeCallTerminate),
				data: unsafe.Pointer(cCallTerm),
			}
			C.callEventCallback(eventHandler, &cevent)
			C.free(unsafe.Pointer(cCallTerm))

		case *events.Message:
			HandleMessage(evt.Info, evt.Message, false)

		case *events.Receipt:

			receiptKind := -1
			if evt.Type == types.ReceiptTypeRead || evt.Type == types.ReceiptTypeReadSelf {
				receiptKind = 0
			}

			if receiptKind != -1 {
				LOG_DEBUG("%#v was read by %s at %s", evt.MessageIDs, evt.SourceString(), evt.Timestamp)
				n := len(evt.MessageIDs)
				cmessageIds := (**C.char)(C.malloc(C.size_t(n) * C.size_t(unsafe.Sizeof(uintptr(0)))))
				messageIds := unsafe.Slice(cmessageIds, len(evt.MessageIDs))
				for i, id := range evt.MessageIDs {
					messageIds[i] = C.CString(id)
				}

				cchatId := jidToC(evt.MessageSource.Chat)

				creceipt := (*C.ReceiptEvent)(C.malloc(C.sizeof_ReceiptEvent))
				creceipt.kind = C.uint8_t(EventTypeReceipt)
				creceipt.id = cchatId
				creceipt.messageIDs = cmessageIds
				creceipt.size = C.size_t(n)

				cevent := C.Event{
					kind: C.uint8_t(EventTypeReceipt),
					data: unsafe.Pointer(creceipt),
				}
				C.callEventCallback(eventHandler, &cevent)
			}

		case *events.HistorySync:
			selfJid := *client.Store.ID

			percent := evt.Data.GetProgress()
			cpercent := (*C.uint8_t)(C.malloc(C.size_t(unsafe.Sizeof(uint8(0)))))
			*cpercent = C.uint8_t(percent)

			cevent := C.Event{
				kind: C.uint8_t(EventTypeSyncProgress),
				data: unsafe.Pointer(cpercent),
			}

			C.callEventCallback(eventHandler, &cevent)

			C.free(unsafe.Pointer(cpercent))

			conversations := evt.Data.GetConversations()
			for _, conversation := range conversations {
				chatJid, _ := types.ParseJID(conversation.GetID())
				syncMessages := conversation.GetMessages()

				for _, syncMessage := range syncMessages {
					webMessageInfo := syncMessage.Message
					messageInfo := ParseWebMessageInfo(selfJid, chatJid, webMessageInfo)
					message := webMessageInfo.GetMessage()

					if (messageInfo == nil) || (message == nil) {
						continue
					}

					HandleMessage(*messageInfo, message, true)
				}
			}
		}
	})
}

//export C_Connect
func C_Connect(handler C.QrCallback, data unsafe.Pointer) {
	if client.Store.ID == nil {
		qrChan, _ = client.GetQRChannel(context.Background())
		err := client.Connect()
		if err != nil {
			panic(err)
		}

		for evt := range qrChan {
			if evt.Event == "code" {
				code := C.CString(evt.Code)
				defer C.free(unsafe.Pointer(code))
				C.callQrCallback(handler, code, data)
			}
		}
	} else {
		err := client.Connect()
		if err != nil {
			panic(err)
		}
	}

	AddEventHandlers()
}

//export C_PairPhone
func C_PairPhone(phone *C.char) *C.char {
	goPhone := C.GoString(phone)
	code, err := client.PairPhone(context.Background(), goPhone, true, whatsmeow.PairClientChrome, "Chrome (Linux)")
	if err != nil {
		panic(err)
	}
	cCode := C.CString(code)
	return cCode
}

//export C_SendMessage
func C_SendMessage(cjid C.JID, messageType C.uint8_t, messageContent unsafe.Pointer, quoteId *C.char, quoteSender C.JID) {
	jid := cToJid(cjid)

	contextInfo := &waE2E.ContextInfo{}
	if quoteId != nil {
		id := C.GoString(quoteId)
		contextInfo.StanzaID = &id
		sender := C.GoString(quoteSender)
		contextInfo.Participant = &sender
	}

	message := ContentToWaE2EMessage(messageType, messageContent, contextInfo)

	sendResponse, err := client.SendMessage(context.Background(), jid, message)
	if err != nil {
		panic(err)
	} else {
		var messageInfo types.MessageInfo
		messageInfo.Chat = jid
		messageInfo.IsFromMe = true
		messageInfo.Sender = *client.Store.ID

		messageInfo.ID = sendResponse.ID
		messageInfo.Timestamp = sendResponse.Timestamp

		LOG_INFO("Message sent: %s %s", messageInfo.ID, messageInfo.Chat)
		HandleMessage(messageInfo, message, false)
	}
}

// TODO: Free the memory allocated for C.JID and C.Contact

//export C_GetContacts
func C_GetContacts() C.GetContactsResult {
	ctx := context.Background()
	var entries []C.ContactEntry

	// Contacts (with LID aliases so group senders keyed by LID resolve to a name).
	contacts, err := client.Store.Contacts.GetAllContacts(ctx)
	if err != nil {
		panic(err)
	}
	for jid, contact := range contacts {
		name := contactDisplayName(contact)
		if name == "" {
			continue
		}
		cName := C.CString(name)
		entries = append(entries, C.ContactEntry{jid: jidToC(jid), name: cName})
		if jid.Server != types.HiddenUserServer {
			if lid, _ := client.Store.LIDs.GetLIDForPN(ctx, jid); !lid.IsEmpty() {
				entries = append(entries, C.ContactEntry{jid: jidToC(lid), name: cName})
			}
		}
	}

	// Groups.
	groups, err := client.GetJoinedGroups(ctx)
	if err != nil {
		panic(err)
	}
	for _, group := range groups {
		entries = append(entries, C.ContactEntry{
			jid:  jidToC(group.JID),
			name: C.CString(group.GroupName.Name),
		})
	}

	n := len(entries)
	c_entries := C.malloc(C.size_t(n) * C.size_t(unsafe.Sizeof(C.ContactEntry{})))
	entryList := unsafe.Slice((*C.ContactEntry)(c_entries), n)
	for i := range n {
		entryList[i] = entries[i]
	}

	return C.GetContactsResult{
		entries: (*C.ContactEntry)(c_entries),
		size:    C.uint32_t(n),
	}
}

//export C_GetChatSettings
func C_GetChatSettings(cjid C.JID) C.ChatSettings {
	ctx := context.Background()
	jid := cToJid(cjid).ToNonAD()

	settings, err := client.Store.ChatSettings.GetChatSettings(ctx, jid)
	if err != nil {
		LOG_WARN("failed to get chat settings for %s: %v", jid, err)
		return C.ChatSettings{}
	}

	if !settings.Found {
		if jid.Server == types.DefaultUserServer {
			// App state often stores settings under LID JID, try that if PN lookup missed.
			if lidJID, mapErr := client.Store.LIDs.GetLIDForPN(ctx, jid); mapErr == nil && !lidJID.IsEmpty() {
				if altSettings, altErr := client.Store.ChatSettings.GetChatSettings(ctx, lidJID.ToNonAD()); altErr == nil && altSettings.Found {
					settings = altSettings
				}
			}
		} else if jid.Server == types.HiddenUserServer {
			if pnJID, mapErr := client.Store.LIDs.GetPNForLID(ctx, jid); mapErr == nil && !pnJID.IsEmpty() {
				if altSettings, altErr := client.Store.ChatSettings.GetChatSettings(ctx, pnJID.ToNonAD()); altErr == nil && altSettings.Found {
					settings = altSettings
				}
			}
		}
	}

	mutedUntil := int64(0)
	if !settings.MutedUntil.IsZero() {
		mutedUntil = settings.MutedUntil.Unix()
	}

	return C.ChatSettings{
		found:       C.bool(settings.Found),
		muted_until: C.int64_t(mutedUntil),
		pinned:      C.bool(settings.Pinned),
		archived:    C.bool(settings.Archived),
	}
}

//export C_MarkAsRead
func C_MarkAsRead(msgs_id **C.char, num_msg C.size_t, chat_jid C.JID, sender_jid C.JID) {
	ctx := context.Background()
	timeRead := time.Now()

	slice := unsafe.Slice(msgs_id, num_msg)

	msgIds := []types.MessageID{}
	for _, msg := range slice {
		msgId := C.GoString(msg)
		msgIds = append(msgIds, types.MessageID(msgId))
	}

	client.MarkRead(
		ctx,
		msgIds,
		timeRead,
		cToJid(chat_jid),
		cToJid(sender_jid),
	)
}

//export C_Disconnect
func C_Disconnect() {
	client.Disconnect()
}

func main() {} // Required for CGO
