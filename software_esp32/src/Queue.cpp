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


#include "Queue.h"

CQueue Queue;      

CQueue::CQueue(byte bufferSize) {
  m_enabled = true;
  m_bufferSize = bufferSize;
  m_mutex = xSemaphoreCreateMutexStatic(&m_mutexBuffer);
}

void CQueue::clear() {
  xSemaphoreTake(m_mutex, portMAX_DELAY);
  while (!m_queue.IsEmpty()) {
    m_queue.Pop();
  }
  xSemaphoreGive(m_mutex);
}

void CQueue::setBufferSize(byte size) {
  m_bufferSize = size;
}

void CQueue::disable() {
  m_enabled = false;
  clear();
}

void CQueue::enable() {
  m_enabled = true;
}

bool CQueue::isEnabled() {
  return m_enabled;
}

void CQueue::push(String data) {
  xSemaphoreTake(m_mutex, portMAX_DELAY);
  if (m_enabled && m_queue.Count() < m_bufferSize) {
    m_queue.Push(data);
  }
  xSemaphoreGive(m_mutex);
}


int CQueue::available()  {
  xSemaphoreTake(m_mutex, portMAX_DELAY);
  int count = m_queue.Count();
  xSemaphoreGive(m_mutex);
  return count;
}

String CQueue::pop() {
  String data;
  xSemaphoreTake(m_mutex, portMAX_DELAY);
  if (!m_queue.IsEmpty()) data = m_queue.Pop();
  xSemaphoreGive(m_mutex);
  return data;
}