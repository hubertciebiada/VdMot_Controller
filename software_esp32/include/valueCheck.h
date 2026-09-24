/**HEADER*******************************************************************
  project : VdMot Controller

  author : SurfGargano, Lenti84

  Comments:

  Version :

  Modifcations :


***************************************************************************
*
* THIS SOFTWARE IS PROVIDED "AS IS" AND ANY EXPRESSED OR
* IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
* OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
* IN NO EVENT SHALL THE DEVELOPER OR ANY CONTRIBUTORS BE LIABLE FOR ANY DIRECT,
* INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
* (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
* SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
* HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
* STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
* IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF
* THE POSSIBILITY OF SUCH DAMAGE.
*
**************************************************************************
  This program is free software: you can redistribute it and/or modify
  it under the terms of the GNU General Public License as published by
  the Free Software Foundation, either version 3 of the License.
  See the GNU General Public License for more details.

  You should have received a copy of the GNU General Public License
  along with this program.  If not, see <http://www.gnu.org/licenses/>.

  Copyright (C) 2021 Lenti84  https://github.com/Lenti84/VdMot_Controller

*END************************************************************************/


#pragma once

// Pure value parsing and range checks, free of Arduino headers so that they
// can be unit tested on the host (test/native).

#include <stdint.h>

// Temperatures are handled in 0.1 degree steps; readings at or below this
// value are STM sentinels (-500 no value, -1270 read error), never temperatures.
#define TEMP_SENTINEL_MAX   (-500)
// a sensor offset is limited to +/- 10.0 degrees (in 0.1 degree steps)
#define TEMP_OFFSET_MAX     100

// Parses a decimal number ("50", " 50.5 ", "5e1"); leading and trailing
// white space is allowed, anything else (empty, hex, inf, nan, junk) is rejected.
bool parseDouble(const char* s, double* value);

// Converts to long, truncating a fractional part; false when not finite or
// out of range for long.
bool doubleToLong(double d, long* value);

// Parses an integer or a decimal number (truncated), see parseDouble.
bool parseLong(const char* s, long* value);

// Converts a sensor offset in degrees to 0.1 degree steps, clamped to
// +/- TEMP_OFFSET_MAX; false when not finite.
bool tempOffsetToTenths(double degrees, int* tenths);

// Adds an offset (0.1 degree steps, clamped to +/- TEMP_OFFSET_MAX, so an
// offset stored by an older firmware is bounded too) to a reading. Sentinels
// pass unchanged, and a valid reading never becomes a sentinel or wraps.
int16_t addTempOffset(int16_t value, int offset);
