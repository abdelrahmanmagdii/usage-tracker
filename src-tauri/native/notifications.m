#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>

enum {
  UBStatusNotDetermined = 0,
  UBStatusDenied = 1,
  UBStatusAuthorized = 2,
  UBStatusUnavailable = 3,
  UBStatusTimeout = -1,
};

enum {
  UBPostOk = 0,
  UBPostInvalid = 1,
  UBPostNotAuthorized = 2,
  UBPostUnavailable = 3,
  UBPostFailed = 4,
  UBPostTimeout = 5,
};

static const int64_t UBShortTimeoutSeconds = 8;
static const int64_t UBPermissionTimeoutSeconds = 120;

@interface UBNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@end

@implementation UBNotificationDelegate
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completionHandler {
  // Use Foundation's version check: the Rust release linker does not link
  // Clang's availability runtime. The presentation options are enum values.
  NSOperatingSystemVersion bigSur = {11, 0, 0};
  if ([NSProcessInfo.processInfo isOperatingSystemAtLeastVersion:bigSur]) {
    completionHandler(UNNotificationPresentationOptionList | UNNotificationPresentationOptionBanner);
  } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    completionHandler(UNNotificationPresentationOptionAlert);
#pragma clang diagnostic pop
  }
}
@end

static UBNotificationDelegate *ub_delegate = nil;

static BOOL ub_is_app_bundle(void) {
  NSURL *url = NSBundle.mainBundle.bundleURL;
  return url != nil && [url.pathExtension isEqualToString:@"app"];
}

static UNUserNotificationCenter *ub_center(void) {
  if (!ub_is_app_bundle()) return nil;
  UNUserNotificationCenter *center = [UNUserNotificationCenter currentNotificationCenter];
  @synchronized([UBNotificationDelegate class]) {
    if (center.delegate == nil) {
      if (ub_delegate == nil) ub_delegate = [UBNotificationDelegate new];
      center.delegate = ub_delegate;
    }
  }
  return center;
}

static dispatch_time_t ub_deadline(int64_t seconds) {
  return dispatch_time(DISPATCH_TIME_NOW, seconds * NSEC_PER_SEC);
}

static int ub_map_status(UNAuthorizationStatus status) {
  switch (status) {
    case UNAuthorizationStatusNotDetermined:
      return UBStatusNotDetermined;
    case UNAuthorizationStatusDenied:
      return UBStatusDenied;
    case UNAuthorizationStatusAuthorized:
    case UNAuthorizationStatusProvisional:
      return UBStatusAuthorized;
    default:
      if ((NSInteger)status == 4) return UBStatusAuthorized;
      return UBStatusUnavailable;
  }
}

static int ub_status_on_worker(UNUserNotificationCenter *center) {
  __block int result = UBStatusUnavailable;
  dispatch_semaphore_t done = dispatch_semaphore_create(0);
  [center getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
    result = ub_map_status(settings.authorizationStatus);
    dispatch_semaphore_signal(done);
  }];
  if (dispatch_semaphore_wait(done, ub_deadline(UBShortTimeoutSeconds)) != 0) return UBStatusTimeout;
  return result;
}

int ub_notification_status(void) {
  @autoreleasepool {
    @try {
      UNUserNotificationCenter *center = ub_center();
      if (center == nil) return UBStatusUnavailable;
      return ub_status_on_worker(center);
    } @catch (NSException *exception) {
      return UBStatusUnavailable;
    }
  }
}

int ub_notification_request(void) {
  @autoreleasepool {
    @try {
      UNUserNotificationCenter *center = ub_center();
      if (center == nil) return UBStatusUnavailable;
      int current = ub_status_on_worker(center);
      if (current != UBStatusNotDetermined) return current;
      __block BOOL granted = NO;
      __block BOOL failed = NO;
      dispatch_semaphore_t done = dispatch_semaphore_create(0);
      UNAuthorizationOptions options = UNAuthorizationOptionAlert | UNAuthorizationOptionSound;
      [center requestAuthorizationWithOptions:options
                            completionHandler:^(BOOL ok, NSError *error) {
                              granted = ok;
                              failed = error != nil;
                              dispatch_semaphore_signal(done);
                            }];
      if (dispatch_semaphore_wait(done, ub_deadline(UBPermissionTimeoutSeconds)) != 0) return UBStatusTimeout;
      if (granted) return UBStatusAuthorized;
      if (failed) return UBStatusUnavailable;
      return UBStatusDenied;
    } @catch (NSException *exception) {
      return UBStatusUnavailable;
    }
  }
}

int ub_notification_post(const char *title, const char *body) {
  @autoreleasepool {
    @try {
      if (title == NULL || body == NULL) return UBPostInvalid;
      NSString *titleText = [NSString stringWithUTF8String:title];
      NSString *bodyText = [NSString stringWithUTF8String:body];
      if (titleText == nil || bodyText == nil) return UBPostInvalid;
      UNUserNotificationCenter *center = ub_center();
      if (center == nil) return UBPostUnavailable;
      int status = ub_status_on_worker(center);
      if (status == UBStatusTimeout) return UBPostTimeout;
      if (status == UBStatusUnavailable) return UBPostUnavailable;
      if (status != UBStatusAuthorized) return UBPostNotAuthorized;

      UNMutableNotificationContent *content = [UNMutableNotificationContent new];
      content.title = titleText;
      content.body = bodyText;
      UNNotificationRequest *request =
          [UNNotificationRequest requestWithIdentifier:NSUUID.UUID.UUIDString content:content trigger:nil];
      __block BOOL failed = NO;
      dispatch_semaphore_t done = dispatch_semaphore_create(0);
      [center addNotificationRequest:request
               withCompletionHandler:^(NSError *error) {
                 failed = error != nil;
                 dispatch_semaphore_signal(done);
               }];
      if (dispatch_semaphore_wait(done, ub_deadline(UBShortTimeoutSeconds)) != 0) return UBPostTimeout;
      return failed ? UBPostFailed : UBPostOk;
    } @catch (NSException *exception) {
      return UBPostFailed;
    }
  }
}
