// openxr_engine.h — Motor OpenXR estéreo para Pico 4 Ultra
#pragma once

#include <android/native_activity.h>

void start_openxr_thread(ANativeActivity* activity);
void stop_openxr_thread();
void pump_android_input_queue();
