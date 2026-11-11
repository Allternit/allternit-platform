# Native board: the unchanged firmware built as a host program.
# See docs/research-allternit-link-native-board-PLAN.md (step 2).
#
#   make all                       three variants into $(OUT)/
#   make FIRMWARE_DIR=/tmp/x OUT=/tmp/x-build link-board
#
# Variants (decision 9):
#   link-board       as shipped: NullDisplay, no dock or stop pin
#   link-board-dock  DOCK_DETECT_PIN=1, STOP_PIN=2
#   link-board-epd   LINK_DISPLAY_SSD1681 with the EPD_* pins below
# Every object and binary is written under a temporary name and renamed, so
# parallel builds into the same OUT do not see half-written files.

FIRMWARE_DIR ?= ../firmware/usb_agent
OUT ?= build
CXX ?= c++

# OpenSSL 3: Homebrew's openssl@3 on macOS (searched first, so a newer
# Homebrew openssl does not win), the system libssl-dev on Linux. Set inline:
# GNU make 3.81 (macOS) does not pass exported variables to $(shell).
OPENSSL_PC_PATH := /opt/homebrew/opt/openssl@3/lib/pkgconfig:/usr/local/opt/openssl@3/lib/pkgconfig$(if $(PKG_CONFIG_PATH),:$(PKG_CONFIG_PATH))
OPENSSL_CFLAGS := $(shell PKG_CONFIG_PATH='$(OPENSSL_PC_PATH)' pkg-config --cflags openssl 2>/dev/null)
OPENSSL_LIBS := $(shell PKG_CONFIG_PATH='$(OPENSSL_PC_PATH)' pkg-config --libs openssl 2>/dev/null || echo -lssl -lcrypto)

CXXFLAGS ?= -O1 -g
WARN := -Wall -Wno-unused-function -Wno-unused-variable -Wno-unused-parameter
# -funsigned-char: Xtensa char is unsigned (decision 2).
BASEFLAGS := -std=gnu++2b -funsigned-char -pthread $(WARN) -Ishim -I$(FIRMWARE_DIR) $(OPENSSL_CFLAGS)
LDLIBS := $(OPENSSL_LIBS) -pthread

VARIANTS := link-board link-board-dock link-board-epd
FLAGS_link-board :=
FLAGS_link-board-dock := -DDOCK_DETECT_PIN=1 -DSTOP_PIN=2
FLAGS_link-board-epd := -DLINK_DISPLAY_SSD1681 -DEPD_SCK_PIN=7 -DEPD_MOSI_PIN=9 -DEPD_CS_PIN=3 -DEPD_DC_PIN=4 -DEPD_RST_PIN=5 -DEPD_BUSY_PIN=6

RUNTIME_SRCS := $(wildcard runtime/*.cpp)
RUNTIME_OBJS := $(patsubst runtime/%.cpp,$(OUT)/obj/runtime/%.o,$(RUNTIME_SRCS))
SHIM_HDRS := $(shell find shim -name '*.h')
FIRMWARE_SRCS := $(wildcard $(FIRMWARE_DIR)/*.ino $(FIRMWARE_DIR)/*.h)

.PHONY: all clean $(VARIANTS)
# Keep the objects (they are intermediate files to make).
.SECONDARY:
all: $(VARIANTS)

$(VARIANTS): %: $(OUT)/%

$(OUT)/obj/runtime/%.o: runtime/%.cpp $(SHIM_HDRS) Makefile
	@mkdir -p $(dir $@)
	$(CXX) $(BASEFLAGS) $(CXXFLAGS) -c $< -o $@.tmp.$$$$ && mv -f $@.tmp.$$$$ $@

$(OUT)/obj/%/board_main.o: board_main.cpp $(FIRMWARE_SRCS) $(SHIM_HDRS) Makefile
	@mkdir -p $(dir $@)
	$(CXX) $(BASEFLAGS) $(FLAGS_$*) $(CXXFLAGS) -c $< -o $@.tmp.$$$$ && mv -f $@.tmp.$$$$ $@

$(OUT)/%: $(OUT)/obj/%/board_main.o $(RUNTIME_OBJS)
	$(CXX) $^ $(LDLIBS) -o $@.tmp.$$$$ && mv -f $@.tmp.$$$$ $@

clean:
	rm -rf $(OUT)
