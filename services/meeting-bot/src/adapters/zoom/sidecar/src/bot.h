// Meeting SDK for Linux wrapper: one bot, one meeting.
//
// SDK names used here come from Zoom's Linux SDK headers as used by Zoom's own
// headless sample (github.com/zoom/meetingsdk-headless-linux-sample, MIT) and
// the Linux raw-data guide
// (https://developers.zoom.us/docs/meeting-sdk/linux/custom-ui/advanced-features/raw-data/).
// Event interfaces gain pure virtual methods between SDK versions. Methods we
// are sure of are marked `override`; the rest are declared without it, so an
// SDK that lacks one still compiles, and one that adds a new pure virtual fails
// at build time with the exact name to add (see ../README.md "Building").
#pragma once

#include <atomic>
#include <condition_variable>
#include <deque>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "zoom_sdk.h"
#include "auth_service_interface.h"
#include "meeting_service_interface.h"
#include "setting_service_interface.h"
#include "meeting_service_components/meeting_audio_interface.h"
#include "meeting_service_components/meeting_chat_interface.h"
#include "meeting_service_components/meeting_participants_ctrl_interface.h"
#include "meeting_service_components/meeting_recording_interface.h"
#include "meeting_service_components/meeting_video_interface.h"
#include "rawdata/zoom_rawdata_api.h"
#include "rawdata/rawdata_audio_helper_interface.h"
#include "rawdata/rawdata_video_source_helper_interface.h"
#include "zoom_sdk_raw_data_def.h"

#include "ipc.h"

namespace allternit {

using namespace ZOOMSDK;

class ZoomBot;

struct JoinArgs {
  std::string sdkJwt;
  std::string meetingNumber;  // decimal, may be empty when vanityId is set
  std::string vanityId;
  std::string passcode;
  std::string displayName;
  std::string obfToken;
  std::string appPrivilegeToken;
};

// ---- SDK event sinks (each forwards to ZoomBot) ---------------------------------

class AuthEvents : public IAuthServiceEvent {
 public:
  explicit AuthEvents(ZoomBot& b) : bot_(b) {}
  void onAuthenticationReturn(AuthResult ret) override;
  void onLoginReturnWithReason(LOGINSTATUS, IAccountInfo*, LoginFailReason) override {}
  void onLogout() override {}
  void onZoomIdentityExpired() override {}
  void onZoomAuthIdentityExpired() override {}

 private:
  ZoomBot& bot_;
};

class MeetingEvents : public IMeetingServiceEvent {
 public:
  explicit MeetingEvents(ZoomBot& b) : bot_(b) {}
  void onMeetingStatusChanged(MeetingStatus status, int iResult) override;
  void onMeetingStatisticsWarningNotification(StatisticsWarningType) override {}
  void onMeetingParameterNotification(const MeetingParameter*) override {}
  void onSuspendParticipantsActivities() override {}
  void onAICompanionActiveChangeNotice(bool) override {}
  void onMeetingTopicChanged(const zchar_t*) override {}
  void onMeetingFullToWatchLiveStream(const zchar_t*) override {}
  void onUserNetworkStatusChanged(MeetingComponentType, ConnectionQuality, unsigned int, bool) override {}

 private:
  ZoomBot& bot_;
};

class ChatEvents : public IMeetingChatCtrlEvent {
 public:
  explicit ChatEvents(ZoomBot& b) : bot_(b) {}
  void onChatMsgNotification(IChatMsgInfo* chatMsg, const zchar_t* content) override;
  void onChatStatusChangedNotification(ChatStatus*) {}
  void onChatMsgDeleteNotification(const zchar_t*, SDKChatMessageDeleteType) {}
  void onChatMessageEditNotification(IChatMsgInfo*) {}
  void onShareMeetingChatStatusChanged(bool) {}
  void onFileSendStart(ISDKFileSender*) {}
  void onFileReceived(ISDKFileReceiver*) {}
  void onFileTransferProgress(SDKFileTransferInfo*) {}

 private:
  ZoomBot& bot_;
};

class RecordingEvents : public IMeetingRecordingCtrlEvent {
 public:
  explicit RecordingEvents(ZoomBot& b) : bot_(b) {}
  void onRecordingStatus(RecordingStatus) override {}
  void onCloudRecordingStatus(RecordingStatus) override {}
  void onRecordPrivilegeChanged(bool bCanRec) override;
  void onLocalRecordingPrivilegeRequestStatus(RequestLocalRecordingStatus status) override;
  void onLocalRecordingPrivilegeRequested(IRequestLocalRecordingPrivilegeHandler*) override {}
  void onCloudRecordingStorageFull(time_t) override {}
  void onRequestCloudRecordingResponse(RequestStartCloudRecordingStatus) override {}
  void onStartCloudRecordingRequested(IRequestStartCloudRecordingHandler*) override {}
  void onEnableAndStartSmartRecordingRequested(IRequestEnableAndStartSmartRecordingHandler*) override {}
  void onSmartRecordingEnableActionCallback(ISmartRecordingEnableActionHandler*) override {}
  void onTranscodingStatusChanged(TranscodingStatus, const zchar_t*) override {}

 private:
  ZoomBot& bot_;
};

class AudioSink : public IZoomSDKAudioRawDataDelegate {
 public:
  explicit AudioSink(Ipc& ipc) : ipc_(ipc) {}
  void onMixedAudioRawDataReceived(AudioRawData* data) override;
  void onOneWayAudioRawDataReceived(AudioRawData*, uint32_t) override {}
  void onShareAudioRawDataReceived(AudioRawData*, unsigned int) override {}
  void onOneWayInterpreterAudioRawDataReceived(AudioRawData*, const zchar_t*) override {}

 private:
  Ipc& ipc_;
};

/** Virtual mic: queues PCM from Node and paces it into the SDK in 20 ms chunks on its own thread. */
class VirtualMic : public IZoomSDKVirtualAudioMicEvent {
 public:
  explicit VirtualMic(Ipc& ipc);
  ~VirtualMic();
  void onMicInitialize(IZoomSDKAudioRawDataSender* sender) override;
  void onMicStartSend() override;
  void onMicStopSend() override;
  void onMicUninitialized() override;

  void push(AudioIn&& a);
  void clear();

 private:
  void pump();

  Ipc& ipc_;
  std::mutex mu_;
  std::condition_variable cv_;
  IZoomSDKAudioRawDataSender* sender_ = nullptr;
  bool sending_ = false;
  bool stop_ = false;
  uint32_t rate_ = 16000;
  std::deque<uint8_t> queue_;  // mono s16le at rate_
  std::thread thread_;
};

/** Camera source fed with I420 frames from Node (the Bot Mode tile). */
class TileCamera : public IZoomSDKVideoSource {
 public:
  explicit TileCamera(Ipc& ipc) : ipc_(ipc) {}
  void onInitialize(IZoomSDKVideoSender* sender, IList<VideoSourceCapability>* caps, VideoSourceCapability& suggest) override;
  void onPropertyChange(IList<VideoSourceCapability>* caps, VideoSourceCapability suggest) override;
  void onStartSend() override;
  void onStopSend() override;
  void onUninitialized() override;

  void push(VideoIn&& v);

 private:
  Ipc& ipc_;
  std::mutex mu_;
  IZoomSDKVideoSender* sender_ = nullptr;
  bool sending_ = false;
};

// ---- the bot ----------------------------------------------------------------------

class ZoomBot {
 public:
  explicit ZoomBot(Ipc& ipc);
  ~ZoomBot();

  /** InitSDK + services. Returns false (and logs) on failure. */
  bool init();
  /** Handle one JSON command from Node (main-loop thread). */
  void handle(const nlohmann::json& cmd);
  /** Leave if in a meeting; safe to call twice. */
  void leaveIfActive();
  /** Destroy services and CleanUPSDK. */
  void cleanup();
  /** Leave, then quit the main loop once Zoom confirms (or after 3 s). */
  void shutdown();
  void setLoop(GMainLoop* loop) { loop_ = loop; }
  /** PCM from Node for the virtual mic (paced on the mic thread). */
  void pushMic(AudioIn&& a) { mic_.push(std::move(a)); }
  /** I420 frame from Node for the camera. */
  void pushCamera(VideoIn&& v) { camera_.push(std::move(v)); }
  static std::string sdkVersion();

  // called by sinks (main-loop thread)
  void onAuth(AuthResult ret);
  void onStatus(MeetingStatus status, int iResult);
  void onChat(IChatMsgInfo* msg, const zchar_t* content);
  void onRecordPrivilege(bool canRec);
  void onRecordRequestStatus(RequestLocalRecordingStatus status);

 private:
  void reply(int id, bool ok, const std::string& error = "", const nlohmann::json& data = nullptr);
  void doJoin();
  void onLive();
  void startRaw();
  void stopTimers();
  static gboolean tick(gpointer self);
  nlohmann::json roster(std::vector<std::string>* talking);
  unsigned int selfId();

  Ipc& ipc_;
  IAuthService* auth_ = nullptr;
  IMeetingService* meeting_ = nullptr;
  ISettingService* settings_ = nullptr;
  AuthEvents authEvents_;
  MeetingEvents meetingEvents_;
  ChatEvents chatEvents_;
  RecordingEvents recordingEvents_;
  AudioSink audioSink_;
  VirtualMic mic_;
  TileCamera camera_;

  JoinArgs args_;
  bool authPending_ = false;
  bool authed_ = false;
  bool inited_ = false;
  bool live_ = false;
  bool rawStarted_ = false;
  guint tickSource_ = 0;
  GMainLoop* loop_ = nullptr;
  bool shuttingDown_ = false;
  unsigned tickCount_ = 0;
  std::string lastRoster_;
  std::string lastTalking_;
};

}  // namespace allternit
