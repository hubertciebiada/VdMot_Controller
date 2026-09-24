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


#include "valueCheck.h"
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

static const char* skipSpace(const char* s)
{
  while (isspace((unsigned char)*s)) s++;
  return s;
}

bool parseDouble(const char* s, double* value)
{
  if (s == NULL) return false;
  s = skipSpace(s);
  // strtod would also take hex numbers, "inf" and "nan"
  const char* p = s;
  while ((*p != '\0') && !isspace((unsigned char)*p)) {
    if (!isdigit((unsigned char)*p) && (strchr("+-.eE", *p) == NULL)) return false;
    p++;
  }
  char* end;
  errno = 0;
  double d = strtod(s, &end);
  if ((end == s) || (errno == ERANGE) || !isfinite(d)) return false;
  if (*skipSpace(end) != '\0') return false;
  *value = d;
  return true;
}

bool doubleToLong(double d, long* value)
{
  // the negated test also rejects NaN
  if (!((d > (double)LONG_MIN - 1.0) && (d < (double)LONG_MAX + 1.0))) return false;
  *value = (long)d;
  return true;
}

bool parseLong(const char* s, long* value)
{
  if (s == NULL) return false;
  // integers first: exact beyond the precision of a double
  char* end;
  errno = 0;
  long v = strtol(s, &end, 10);
  if ((end != s) && (errno != ERANGE) && (*skipSpace(end) == '\0')) {
    *value = v;
    return true;
  }
  double d;
  return parseDouble(s, &d) && doubleToLong(d, value);
}

bool tempOffsetToTenths(double degrees, int* tenths)
{
  if (!isfinite(degrees)) return false;
  double t = degrees * 10;
  if (t > TEMP_OFFSET_MAX) t = TEMP_OFFSET_MAX;
  if (t < -TEMP_OFFSET_MAX) t = -TEMP_OFFSET_MAX;
  *tenths = (int)lround(t);
  return true;
}

int16_t addTempOffset(int16_t value, int offset)
{
  if (value <= TEMP_SENTINEL_MAX) return value;
  if (offset > TEMP_OFFSET_MAX) offset = TEMP_OFFSET_MAX;
  if (offset < -TEMP_OFFSET_MAX) offset = -TEMP_OFFSET_MAX;
  long result = (long)value + offset;
  if (result <= TEMP_SENTINEL_MAX) result = TEMP_SENTINEL_MAX + 1;
  if (result > INT16_MAX) result = INT16_MAX;
  return (int16_t)result;
}
