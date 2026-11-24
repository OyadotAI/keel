// Native WK ownership, isolated frame inventory and AppKit input. All entry points run on the UI thread.
#import <WebKit/WebKit.h>
#import <JavaScriptCore/JavaScriptCore.h>
#import <AppKit/AppKit.h>
#include <cmath>
static const NSUInteger maxFrames = 128, maxString = 262144, maxUrl = 4096;
static const char *stale = "Frame changed or closed. Read the page again with frames:true.";
typedef void (*Reply)(void *, const char *, const char *);
@interface KeelDebugFrame : NSObject
@property WKFrameInfo *frame;
@property NSString *document;
@property NSString *reference;
@end
@implementation KeelDebugFrame
@end
@interface KeelDebugHost : NSObject <WKScriptMessageHandler>
@property(weak) WKWebView *view;
@property NSString *key;
@property NSMutableDictionary<NSString *, KeelDebugFrame *> *frames;
@property NSMutableSet<NSValue *> *pending;
@end
static NSMutableDictionary<NSString *, KeelDebugHost *> *hosts;
static NSString *string(const char *s) { return s ? [NSString stringWithUTF8String:s] : @""; }
static void answer(Reply reply, void *context, NSString *value, NSString *error) {
  if (value.length > maxString) { value = nil; error = @"Native diagnostic result exceeded its size limit."; }
  reply(context, value.UTF8String, error.UTF8String);
}
@implementation KeelDebugHost
- (void)userContentController:(WKUserContentController *)controller didReceiveScriptMessage:(WKScriptMessage *)message {
  if (message.webView != self.view || hosts[self.key] != self || ![message.body isKindOfClass:NSString.class]) return;
  NSString *body = message.body;
  if (body.length > 256) return;
  NSDictionary *event = [NSJSONSerialization JSONObjectWithData:[body dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
  NSString *doc = event[@"document"], *kind = event[@"kind"];
  if (![doc isKindOfClass:NSString.class] || doc.length > 128) return;
  for (NSString *ref in self.frames.allKeys) {
    if ([self.frames[ref].document isEqual:doc]) {
      if ([kind isEqual:@"retire"]) [self.frames removeObjectForKey:ref];
      return;
    }
  }
  if (![kind isEqual:@"register"]) return;
  if (message.frameInfo.mainFrame) [self.frames removeAllObjects];
  if (self.frames.count >= maxFrames) return;
  KeelDebugFrame *frame = [KeelDebugFrame new];
  frame.frame = message.frameInfo; frame.document = doc;
  frame.reference = [@"f-" stringByAppendingString:NSUUID.UUID.UUIDString];
  self.frames[frame.reference] = frame;
}
@end
extern "C" void keel_debug_install(void *native, const char *key, const char *runtime, const char *bootstrap) {
  NSCAssert(NSThread.isMainThread, @"diagnostic install must be on UI thread");
  WKWebView *view = (__bridge WKWebView *)native;
  if (!hosts) hosts = [NSMutableDictionary new];
  KeelDebugHost *host = [KeelDebugHost new];
  host.view = view; host.key = string(key); host.frames = [NSMutableDictionary new]; host.pending = [NSMutableSet new]; hosts[host.key] = host;
  WKContentWorld *world = [WKContentWorld worldWithName:@"KeelOwnedDiagnostics"];
  WKUserContentController *controller = view.configuration.userContentController;
  [controller addScriptMessageHandler:host contentWorld:world name:@"keel_debug"];
  [controller addUserScript:[[WKUserScript alloc] initWithSource:string(runtime) injectionTime:WKUserScriptInjectionTimeAtDocumentStart forMainFrameOnly:NO inContentWorld:world]];
  [controller addUserScript:[[WKUserScript alloc] initWithSource:string(bootstrap) injectionTime:WKUserScriptInjectionTimeAtDocumentStart forMainFrameOnly:NO inContentWorld:WKContentWorld.pageWorld]];
}
static KeelDebugHost *owned(void *native, const char *key) {
  KeelDebugHost *host = hosts[string(key)];
  return NSThread.isMainThread && host.view == (__bridge WKWebView *)native ? host : nil;
}
// URLs are display metadata only; exact WKFrameInfo references carry all selection authority.
static NSString *shortUrl(NSString *url) { return url.length > maxUrl ? [[url substringToIndex:maxUrl] stringByAppendingString:@"… [URL shortened]"] : url; }
extern "C" void keel_debug_frames(void *native, const char *key, Reply reply, void *context) {
  KeelDebugHost *host = owned(native,key);
  if (!host) return answer(reply,context,nil,@"The owned browser tab closed.");
  NSMutableArray *rows = [NSMutableArray new];
  for (KeelDebugFrame *frame in host.frames.allValues) {
    [rows addObject:@{@"reference":frame.reference,@"document":frame.document,@"main":@(frame.frame.mainFrame),@"url":shortUrl(frame.frame.request.URL.absoluteString ?: @"")}];
  }
  NSData *data = [NSJSONSerialization dataWithJSONObject:rows options:0 error:nil];
  answer(reply,context,[[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding],nil);
}
extern "C" void keel_debug_run(void *native, const char *key, const char *reference, const char *source, bool page, bool async, Reply reply, void *context) {
  KeelDebugHost *host = owned(native,key);
  KeelDebugFrame *frame = host.frames[string(reference)];
  if (!host || !frame) return answer(reply,context,nil,string(stale));
  NSValue *request = [NSValue valueWithPointer:context];
  if (host.pending.count >= maxFrames) return answer(reply,context,nil,@"Native frame request capacity exceeded; close this owned tab to recover.");
  [host.pending addObject:request];
  WKContentWorld *world = page ? WKContentWorld.pageWorld : [WKContentWorld worldWithName:@"KeelOwnedDiagnostics"];
  void (^done)(id,NSError *) = ^(id value, NSError *error) {
    [host.pending removeObject:request];
    if (hosts[host.key] != host || host.frames[frame.reference] != frame) return answer(reply,context,nil,string(stale));
    if (error) return answer(reply,context,nil,@"The frame rejected JavaScript, changed, or stopped responding. The expression may have run; inspect before retrying.");
    if (![value isKindOfClass:NSString.class]) return answer(reply,context,nil,@"The frame did not return a JSON string.");
    answer(reply,context,value,nil);
  };
  if (async) [host.view callAsyncJavaScript:string(source) arguments:nil inFrame:frame.frame inContentWorld:world completionHandler:done];
  else [host.view evaluateJavaScript:string(source) inFrame:frame.frame inContentWorld:world completionHandler:done];
}
extern "C" void keel_debug_identity(void *native, const char *key, Reply reply, void *context) {
  KeelDebugHost *host = owned(native,key);
  if (!host) return answer(reply,context,nil,@"The owned browser tab closed.");
  WKWebView *view = host.view;
  NSString *engine = [NSBundle bundleForClass:WKWebView.class].infoDictionary[@"CFBundleVersion"];
  NSString *profile = view.configuration.websiteDataStore.identifier.UUIDString;
  NSDictionary *value = @{@"contacted":@YES,@"browser":@"Inside Oya Factory",@"version":NSBundle.mainBundle.infoDictionary[@"CFBundleShortVersionString"] ?: NSNull.null,@"engine":engine ?: NSNull.null,@"profile":@{@"id":profile ?: @"unknown",@"label":@"Project browser profile"},@"dpr":NSNull.null};
  NSData *data = [NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
  answer(reply,context,[[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding],nil);
}
extern "C" void keel_debug_forget(const char *key) { if (NSThread.isMainThread) [hosts removeObjectForKey:string(key)]; }
extern "C" bool keel_debug_parse(const char *body) {
  JSGlobalContextRef context = JSGlobalContextCreate(nullptr);
  JSStringRef source = JSStringCreateWithUTF8CString(body), name = JSStringCreateWithUTF8CString("KeelDebugPayload");
  JSValueRef exception = nullptr;
  JSObjectRef function = JSObjectMakeFunction(context,name,0,nullptr,source,nullptr,1,&exception);
  bool valid = function && !exception;
  JSStringRelease(source); JSStringRelease(name); JSGlobalContextRelease(context);
  return valid;
}
// Presentation classification never scrolls, focuses or queries page JavaScript.
static bool presented(WKWebView *view) {
  return view.window && view.window.visible && !view.window.miniaturized && !view.hiddenOrHasHiddenAncestor && !NSIsEmptyRect(view.visibleRect);
}
extern "C" void keel_debug_presentation(void *native, const char *key, Reply reply, void *context) {
  KeelDebugHost *host=owned(native,key);
  answer(reply,context,host && presented(host.view) ? @"true" : @"false",nil);
}
// Movement belongs to AppKit's tracking owner; button dispatch belongs to the native hit responder.
static id movementReceiver(WKWebView *view, NSView *hit) {
  NSMutableSet *owners = [NSMutableSet new];
  for (NSTrackingArea *area in view.trackingAreas) {
    id owner = area.owner;
    if (owner != view && (area.options & NSTrackingMouseMoved) && [owner respondsToSelector:@selector(mouseMoved:)]) [owners addObject:owner];
  }
  if (owners.count > 1) return nil;
  return owners.anyObject ?: hit;
}
static void dispatchMouse(WKWebView *view, id receiver, NSPoint local, int kind) {
  NSEventType types[] = {NSEventTypeMouseMoved,NSEventTypeLeftMouseDown,NSEventTypeLeftMouseUp};
  NSEvent *event = [NSEvent mouseEventWithType:types[kind] location:[view convertPoint:local toView:nil] modifierFlags:0 timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:view.window.windowNumber context:nil eventNumber:0 clickCount:kind ? 1 : 0 pressure:kind == 1 ? 1.0 : 0.0];
  if (kind == 0) [receiver mouseMoved:event]; else if (kind == 1) [receiver mouseDown:event]; else [receiver mouseUp:event];
}
static void inputReply(Reply reply, void *context, NSMutableArray *events, NSString *error) {
  NSDictionary *result = @{@"ok":@(!error),@"dispatched":events,@"error":error ?: NSNull.null};
  NSData *data = [NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
  answer(reply,context,[[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding],nil);
}
extern "C" void keel_debug_mouse(void *native, const char *key, const char *reference, const char *document, double x, double y, int kind, Reply reply, void *context) {
  KeelDebugHost *host = owned(native,key);
  if (!host || !std::isfinite(x) || !std::isfinite(y) || kind < 0 || kind > 3) return answer(reply,context,nil,@"The owned native input target is unavailable.");
  KeelDebugFrame *frame = host.frames[string(reference)];
  if (!frame || ![frame.document isEqual:string(document)]) return answer(reply,context,nil,string(stale));
  WKWebView *view = host.view;
  double nativeY = y * view.pageZoom + view.safeAreaInsets.top;
  NSPoint local = NSMakePoint(NSMinX(view.bounds) + x * view.pageZoom + view.safeAreaInsets.left, view.flipped ? NSMinY(view.bounds) + nativeY : NSMaxY(view.bounds) - nativeY);
  if (!NSPointInRect(local,view.bounds) || !presented(view)) return answer(reply,context,nil,@"Show the owned Oya Factory window before native input.");
  NSView *hit = [view hitTest:[view convertPoint:local toView:view.superview]];
  if (!hit || hit.window != view.window) return answer(reply,context,nil,@"Native view hit testing refused this click.");
  NSView *content=view.window.contentView;
  NSPoint windowPoint=[view convertPoint:local toView:content.superview];
  NSView *windowHit=[content hitTest:windowPoint];
  if (windowHit != view && ![windowHit isDescendantOf:view]) return answer(reply,context,nil,@"The native page is covered or no longer presented. No further input was sent.");
  id receiver = kind == 0 ? movementReceiver(view,hit) : hit;
  if (!receiver) return answer(reply,context,nil,@"The native movement recipient is ambiguous. No input was sent.");
  NSMutableArray *events = [NSMutableArray new];
  @try {
    if (kind == 0) { [NSApp activateIgnoringOtherApps:YES]; [view.window makeKeyWindow]; [view.window makeFirstResponder:view]; [events addObject:@"native_focus"]; }
    if (kind == 3) {
      dispatchMouse(view,hit,local,1);[events addObject:@"mousedown"];
      dispatchMouse(view,hit,local,2);[events addObject:@"mouseup"];
    } else { dispatchMouse(view,receiver,local,kind); [events addObject:@[@"mousemove",@"mousedown",@"mouseup"][kind]]; }
    inputReply(reply,context,events,nil);
  } @catch (NSException *exception) {
    if ([events containsObject:@"mousedown"] && ![events containsObject:@"mouseup"]) {
      @try { dispatchMouse(view,hit,local,2);[events addObject:@"mouseup_cleanup"]; } @catch (NSException *ignored) {}
    }
    inputReply(reply,context,events,@"Native mouse dispatch failed. Inspect the completed events before retrying.");
  }
}
