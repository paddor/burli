#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]
#![allow(clippy::struct_field_names)]

// Derived from Google's Brotli dictionary transform table.
// Copyright 2013 Google Inc. Distributed under the MIT license.

const kIdentity: u8 = 0;
const kOmitLast1: u8 = 1;
const kOmitLast2: u8 = 2;
const kOmitLast3: u8 = 3;
const kOmitLast4: u8 = 4;
const kOmitLast5: u8 = 5;
const kOmitLast6: u8 = 6;
const kOmitLast7: u8 = 7;
const kOmitLast8: u8 = 8;
const kOmitLast9: u8 = 9;
const kUppercaseFirst: u8 = 10;
const kUppercaseAll: u8 = 11;
const kOmitFirst1: u8 = 12;
const kOmitFirst2: u8 = 13;
const kOmitFirst3: u8 = 14;
const kOmitFirst4: u8 = 15;
const kOmitFirst5: u8 = 16;
const kOmitFirst6: u8 = 17;
const kOmitFirst7: u8 = 18;
// const  kOmitFirst8     : u8 = 19; // <-- unused (reserved)
const kOmitFirst9: u8 = 20;

pub struct Transform {
    pub prefix_id: u8,
    pub transform: u8,
    pub suffix_id: u8,
}

const kPrefixSuffix: [u8; 208] = [
    0x00, 0x20, 0x00, 0x2c, 0x20, 0x00, 0x20, 0x6f, 0x66, 0x20, 0x74, 0x68, 0x65, 0x20, 0x00, 0x20,
    0x6f, 0x66, 0x20, 0x00, 0x73, 0x20, 0x00, 0x2e, 0x00, 0x20, 0x61, 0x6e, 0x64, 0x20, 0x00, 0x20,
    0x69, 0x6e, 0x20, 0x00, 0x22, 0x00, 0x20, 0x74, 0x6f, 0x20, 0x00, 0x22, 0x3e, 0x00, 0x0a, 0x00,
    0x2e, 0x20, 0x00, 0x5d, 0x00, 0x20, 0x66, 0x6f, 0x72, 0x20, 0x00, 0x20, 0x61, 0x20, 0x00, 0x20,
    0x74, 0x68, 0x61, 0x74, 0x20, 0x00, 0x27, 0x00, 0x20, 0x77, 0x69, 0x74, 0x68, 0x20, 0x00, 0x20,
    0x66, 0x72, 0x6f, 0x6d, 0x20, 0x00, 0x20, 0x62, 0x79, 0x20, 0x00, 0x28, 0x00, 0x2e, 0x20, 0x54,
    0x68, 0x65, 0x20, 0x00, 0x20, 0x6f, 0x6e, 0x20, 0x00, 0x20, 0x61, 0x73, 0x20, 0x00, 0x20, 0x69,
    0x73, 0x20, 0x00, 0x69, 0x6e, 0x67, 0x20, 0x00, 0x0a, 0x09, 0x00, 0x3a, 0x00, 0x65, 0x64, 0x20,
    0x00, 0x3d, 0x22, 0x00, 0x20, 0x61, 0x74, 0x20, 0x00, 0x6c, 0x79, 0x20, 0x00, 0x2c, 0x00, 0x3d,
    0x27, 0x00, 0x2e, 0x63, 0x6f, 0x6d, 0x2f, 0x00, 0x2e, 0x20, 0x54, 0x68, 0x69, 0x73, 0x20, 0x00,
    0x20, 0x6e, 0x6f, 0x74, 0x20, 0x00, 0x65, 0x72, 0x20, 0x00, 0x61, 0x6c, 0x20, 0x00, 0x66, 0x75,
    0x6c, 0x20, 0x00, 0x69, 0x76, 0x65, 0x20, 0x00, 0x6c, 0x65, 0x73, 0x73, 0x20, 0x00, 0x65, 0x73,
    0x74, 0x20, 0x00, 0x69, 0x7a, 0x65, 0x20, 0x00, 0xc2, 0xa0, 0x00, 0x6f, 0x75, 0x73, 0x20, 0x00,
];
//    "\0 \0, \0 of the \0 of \0s \0.\0 and \0 in \0\"\0 to \0\">\0\n\0. \0]\0"
//    " for \0 a \0 that \0\'\0 with \0 from \0 by \0(\0. The \0 on \0 as \0"
//    " is \0ing \0\n\t\0:\0ed \0=\"\0 at \0ly \0,\0=\'\0.com/\0. This \0"
//    " not \0er \0al \0ful \0ive \0less \0est \0ize \0\xc2\xa0\0ous ";

// EMPTY = ""
// SP = " "
// DQUOT = "\""
// SQUOT = "'"
// CLOSEBR = "]"
// OPEN = "("
// SLASH = "/"
// NBSP = non-breaking space "\0xc2\xa0"
//
const kPFix_EMPTY: u8 = 0;
const kPFix_SP: u8 = 1;
const kPFix_COMMASP: u8 = 3;
const kPFix_SPofSPtheSP: u8 = 6;
const kPFix_SPtheSP: u8 = 9;
const kPFix_eSP: u8 = 12;
const kPFix_SPofSP: u8 = 15;
const kPFix_sSP: u8 = 20;
const kPFix_DOT: u8 = 23;
const kPFix_SPandSP: u8 = 25;
const kPFix_SPinSP: u8 = 31;
const kPFix_DQUOT: u8 = 36;
const kPFix_SPtoSP: u8 = 38;
const kPFix_DQUOTGT: u8 = 43;
const kPFix_NEWLINE: u8 = 46;
const kPFix_DOTSP: u8 = 48;
const kPFix_CLOSEBR: u8 = 51;
const kPFix_SPforSP: u8 = 53;
const kPFix_SPaSP: u8 = 59;
const kPFix_SPthatSP: u8 = 63;
const kPFix_SQUOT: u8 = 70;
const kPFix_SPwithSP: u8 = 72;
const kPFix_SPfromSP: u8 = 79;
const kPFix_SPbySP: u8 = 86;
const kPFix_OPEN: u8 = 91;
const kPFix_DOTSPTheSP: u8 = 93;
const kPFix_SPonSP: u8 = 100;
const kPFix_SPasSP: u8 = 105;
const kPFix_SPisSP: u8 = 110;
const kPFix_ingSP: u8 = 115;
const kPFix_NEWLINETAB: u8 = 120;
const kPFix_COLON: u8 = 123;
const kPFix_edSP: u8 = 125;
const kPFix_EQDQUOT: u8 = 129;
const kPFix_SPatSP: u8 = 132;
const kPFix_lySP: u8 = 137;
const kPFix_COMMA: u8 = 141;
const kPFix_EQSQUOT: u8 = 143;
const kPFix_DOTcomSLASH: u8 = 146;
const kPFix_DOTSPThisSP: u8 = 152;
const kPFix_SPnotSP: u8 = 160;
const kPFix_erSP: u8 = 166;
const kPFix_alSP: u8 = 170;
const kPFix_fulSP: u8 = 174;
const kPFix_iveSP: u8 = 179;
const kPFix_lessSP: u8 = 184;
const kPFix_estSP: u8 = 190;
const kPFix_izeSP: u8 = 195;
const kPFix_NBSP: u8 = 200;
const kPFix_ousSP: u8 = 203;

pub const kNumTransforms: i32 = 121;
pub const kTransforms: [Transform; kNumTransforms as usize] = [
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitFirst1,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPtheSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_sSP,
        transform: kIdentity,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPofSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPandSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitFirst2,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast1,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_COMMASP,
        transform: kIdentity,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_COMMASP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseFirst,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPinSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPtoSP,
    },
    Transform {
        prefix_id: kPFix_eSP,
        transform: kIdentity,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_DQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_DOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_DQUOTGT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_NEWLINE,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast3,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_CLOSEBR,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPforSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitFirst3,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast2,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPaSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPthatSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseFirst,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_DOTSP,
    },
    Transform {
        prefix_id: kPFix_DOT,
        transform: kIdentity,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_COMMASP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitFirst4,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPwithSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPfromSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPbySP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitFirst5,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitFirst6,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_SPtheSP,
        transform: kIdentity,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast4,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_DOTSPTheSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPonSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPasSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPisSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast7,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast1,
        suffix_id: kPFix_ingSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_NEWLINETAB,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_COLON,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_DOTSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_edSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitFirst9,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitFirst7,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast6,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_OPEN,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_COMMASP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast8,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPatSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_lySP,
    },
    Transform {
        prefix_id: kPFix_SPtheSP,
        transform: kIdentity,
        suffix_id: kPFix_SPofSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast5,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kOmitLast9,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseFirst,
        suffix_id: kPFix_COMMASP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_DQUOT,
    },
    Transform {
        prefix_id: kPFix_DOT,
        transform: kIdentity,
        suffix_id: kPFix_OPEN,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_DQUOTGT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_EQDQUOT,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_DOT,
    },
    Transform {
        prefix_id: kPFix_DOTcomSLASH,
        transform: kIdentity,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_SPtheSP,
        transform: kIdentity,
        suffix_id: kPFix_SPofSPtheSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_SQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_DOTSPThisSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_COMMA,
    },
    Transform {
        prefix_id: kPFix_DOT,
        transform: kIdentity,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_OPEN,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_DOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_SPnotSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_EQDQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_erSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseAll,
        suffix_id: kPFix_SP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_alSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseAll,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_EQSQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_DQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_DOTSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_OPEN,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_fulSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseFirst,
        suffix_id: kPFix_DOTSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_iveSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_lessSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_SQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_estSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseFirst,
        suffix_id: kPFix_DOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_DQUOTGT,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_EQSQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_COMMA,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_izeSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_DOT,
    },
    Transform {
        prefix_id: kPFix_NBSP,
        transform: kIdentity,
        suffix_id: kPFix_EMPTY,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kIdentity,
        suffix_id: kPFix_COMMA,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_EQDQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_EQDQUOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kIdentity,
        suffix_id: kPFix_ousSP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_COMMASP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseFirst,
        suffix_id: kPFix_EQSQUOT,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseFirst,
        suffix_id: kPFix_COMMA,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseAll,
        suffix_id: kPFix_EQDQUOT,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseAll,
        suffix_id: kPFix_COMMASP,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_COMMA,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_OPEN,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_DOTSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseAll,
        suffix_id: kPFix_DOT,
    },
    Transform {
        prefix_id: kPFix_EMPTY,
        transform: kUppercaseAll,
        suffix_id: kPFix_EQSQUOT,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseAll,
        suffix_id: kPFix_DOTSP,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseFirst,
        suffix_id: kPFix_EQDQUOT,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseAll,
        suffix_id: kPFix_EQSQUOT,
    },
    Transform {
        prefix_id: kPFix_SP,
        transform: kUppercaseFirst,
        suffix_id: kPFix_EQSQUOT,
    },
];

fn uppercase_one(bytes: &mut [u8]) -> Option<usize> {
    let first = *bytes.first()?;
    if first < 0xc0 {
        if bytes[0].is_ascii_lowercase() {
            bytes[0] ^= 32;
        }
        return Some(1);
    }
    if first < 0xe0 {
        *bytes.get_mut(1)? ^= 32;
        return Some(2);
    }
    *bytes.get_mut(2)? ^= 5;
    Some(3)
}

/// Longest prefix or suffix, in bytes.
const AFFIX_BYTES: usize = 8;

/// Longest dictionary word, in bytes.
const WORD_BYTES: usize = 24;

/// Bytes a transformed word writes with fixed-size windows: a prefix
/// window, the word window after a prefix of at most [`AFFIX_BYTES`], and a
/// suffix window after the word.
const WINDOW_BYTES: usize = 2 * AFFIX_BYTES + WORD_BYTES;

/// A transform with its prefix and suffix inline, so applying it needs no
/// scan for their ends.
#[derive(Clone, Copy)]
struct PackedTransform {
    prefix: [u8; AFFIX_BYTES],
    suffix: [u8; AFFIX_BYTES],
    prefix_len: u8,
    suffix_len: u8,
    transform: u8,
}

/// The affix at `offset` in [`kPrefixSuffix`], padded with zeros. A longer
/// affix fails to compile.
const fn affix(offset: u8) -> ([u8; AFFIX_BYTES], u8) {
    let mut bytes = [0; AFFIX_BYTES];
    let mut len = 0;
    while kPrefixSuffix[offset as usize + len] != 0 {
        bytes[len] = kPrefixSuffix[offset as usize + len];
        len += 1;
    }
    (bytes, len as u8)
}

static kPackedTransforms: [PackedTransform; kNumTransforms as usize] = {
    let mut packed = [PackedTransform {
        prefix: [0; AFFIX_BYTES],
        suffix: [0; AFFIX_BYTES],
        prefix_len: 0,
        suffix_len: 0,
        transform: 0,
    }; kNumTransforms as usize];
    let mut index = 0;
    while index < packed.len() {
        let (prefix, prefix_len) = affix(kTransforms[index].prefix_id);
        let (suffix, suffix_len) = affix(kTransforms[index].suffix_id);
        packed[index] = PackedTransform {
            prefix,
            suffix,
            prefix_len,
            suffix_len,
            transform: kTransforms[index].transform,
        };
        index += 1;
    }
    packed
};

/// The part of a `word_len`-byte word that `transform` keeps, after its
/// omit-first or omit-last step.
#[inline(always)]
fn kept_range(transform: u8, word_len: usize) -> core::ops::Range<usize> {
    let mut start = 0_usize;
    let mut end = word_len;
    if transform >= kOmitFirst1 {
        start = usize::from(transform - (kOmitFirst1 - 1)).min(end);
    }
    if transform <= kOmitLast9 {
        end = end.saturating_sub(usize::from(transform));
    }
    start.min(end)..end
}

/// A dictionary word under its transform, ready to write.
pub(crate) struct TransformedWord<'a> {
    transform: &'static PackedTransform,
    /// The dictionary from the word on.
    tail: &'a [u8],
    /// The part of the word that the transform keeps.
    kept: core::ops::Range<usize>,
}

impl<'a> TransformedWord<'a> {
    /// The `word_len`-byte word at the start of `tail` under `transform`,
    /// or `None` for an unknown transform.
    #[inline(always)]
    pub(crate) fn new(tail: &'a [u8], word_len: usize, transform: usize) -> Option<Self> {
        let transform = kPackedTransforms.get(transform)?;
        Some(Self {
            transform,
            tail,
            kept: kept_range(transform.transform, word_len),
        })
    }

    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        usize::from(self.transform.prefix_len)
            + self.kept.len()
            + usize::from(self.transform.suffix_len)
    }

    /// Writes the word to `output[pos..]`, which holds at least
    /// [`Self::len`] bytes.
    ///
    /// With room for them, the prefix, word, and suffix move as fixed-size
    /// windows. Each window runs past its part into bytes that the next part
    /// or later output replaces.
    #[inline(always)]
    pub(crate) fn write(&self, output: &mut [u8], pos: usize) -> Option<()> {
        let transform = self.transform;
        let kept = self.kept.clone();
        let prefix_len = usize::from(transform.prefix_len).min(AFFIX_BYTES);
        let kept_len = kept.len().min(WORD_BYTES);
        let window = output
            .get_mut(pos..)
            .and_then(<[u8]>::first_chunk_mut::<WINDOW_BYTES>);
        let source = self
            .tail
            .get(kept.start..)
            .and_then(<[u8]>::first_chunk::<WORD_BYTES>);
        let kept_out = match (window, source) {
            (Some(window), Some(source)) => {
                window[..AFFIX_BYTES].copy_from_slice(&transform.prefix);
                window[prefix_len..prefix_len + WORD_BYTES].copy_from_slice(source);
                let suffix_start = prefix_len + kept_len;
                window[suffix_start..suffix_start + AFFIX_BYTES].copy_from_slice(&transform.suffix);
                &mut window[prefix_len..suffix_start]
            }
            _ => {
                let prefix = &transform.prefix[..prefix_len];
                let suffix =
                    &transform.suffix[..usize::from(transform.suffix_len).min(AFFIX_BYTES)];
                let rest = output.get_mut(pos..)?;
                let (prefix_out, rest) = rest.split_at_mut_checked(prefix.len())?;
                prefix_out.copy_from_slice(prefix);
                let (kept_out, rest) = rest.split_at_mut_checked(kept.len())?;
                kept_out.copy_from_slice(self.tail.get(kept)?);
                rest.get_mut(..suffix.len())?.copy_from_slice(suffix);
                kept_out
            }
        };

        if transform.transform == kUppercaseFirst && !kept_out.is_empty() {
            uppercase_one(kept_out)?;
        } else if transform.transform == kUppercaseAll {
            let mut offset = 0_usize;
            while offset < kept_out.len() {
                offset += uppercase_one(&mut kept_out[offset..])?;
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::uppercase_one;

    #[test]
    fn uppercase_one_rejects_truncated_multibyte_words() {
        assert_eq!(uppercase_one(&mut []), None);
        assert_eq!(uppercase_one(&mut [0xc0]), None);
        assert_eq!(uppercase_one(&mut [0xe0, 0x80]), None);
    }

    #[test]
    fn uppercase_one_handles_complete_words() {
        let mut ascii = *b"a";
        assert_eq!(uppercase_one(&mut ascii), Some(1));
        assert_eq!(ascii, *b"A");

        let mut two_byte = [0xc3, 0xa0];
        assert_eq!(uppercase_one(&mut two_byte), Some(2));
        assert_eq!(two_byte, [0xc3, 0x80]);

        let mut three_byte = [0xe2, 0x82, 0xac];
        assert_eq!(uppercase_one(&mut three_byte), Some(3));
        assert_eq!(three_byte, [0xe2, 0x82, 0xa9]);
    }
}
