// Golden STM reply lines (as an STM sends them, without CR LF): one or more
// per reply form the codec parses. Shared by the codec tests and the glue
// harness (the default replies of the fake STM).
#pragma once

#include <stddef.h>

namespace vdm_test {

inline constexpr const char* kStmGolden[] = {
    "gvlvd 3 42 18 1 215 -500 57 3120 3350 230 0 ",
    "gvlvd 0 0 20 131 -1270 -1270 2000 12000 12000 -12000 2 ",
    "gvlst 12 8,6,6,8,6,6,6,6,6,6,6,6, ",
    "gonec 3 28-84-37-94-97-ff-03-23,28-aa-bb-cc-dd-ee-01-67,26-11-22-33-44-55-66-29 ",
    "gonec 0 ",
    "goned 28-84-37-94-97-ff-03-23 215 ",
    "goned 0 ",
    "goned error ",
    "gvlon 3 28-84-37-94-97-ff-03-23 00-00-00-00-00-00-00-00 ",
    "gowvc 1 26-11-22-33-44-55-66-29 ",
    "gowvd 26-11-22-33-44-55-66-29 1234 ",
    "stgtp",
    "stvls 3",
    "gtgtp 3 50 ",
    "gtlnm 2000 ",
    "gmotc 17 17 50 3000 0 ",
    "gvers 1.4.9_Dev_C2 1712345678 ",
    "ghwin 1073 ",
    "eepst 1 ",
    "gproto 2",
    "gvlvx 4 130 42 60 21 3120 3350 -230 1 57 2 7 3 1 3000 1450 3 412 8123",
    "gprof 3 3 0:150 1500:212 3000:98",
    "svmov 3 err 2",
    "scalx ok",
    "gcalx 1 10 40",
    "gstat 3600 2 4 17 5 1",
    // protocol 3 (STM 2.1)
    "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 66 4 50 50 3540 0",
    "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0",
    "slhbt 1 3540",
    "slhbt err",
    "slcfg ok",
    "sfspo 255 ok",
    "sfspo -1 err 1",
    "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50",
    "sstop 3 ok",
    "gtlnt 604800",
    "ssafe ok",
    "stlnt",
    "svmov -1 err 1",
};
inline constexpr size_t kStmGoldenCount = sizeof kStmGolden / sizeof kStmGolden[0];

}  // namespace vdm_test
